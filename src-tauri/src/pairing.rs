use crate::auth_broker;
use base64::{engine::general_purpose, Engine as _};
use futures_util::{SinkExt, StreamExt};
use http::header::{HeaderName, HeaderValue, AUTHORIZATION};
use rand::RngCore;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::process::Command as StdCommand;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    time::{sleep, timeout, Duration},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
use url::Url;
use uuid::Uuid;

const API_BASE: &str = "https://chatgpt.com/backend-api";
const RELAY_URL: &str = "wss://chatgpt.com/backend-api/codex/remote/control/client";
const AUTH_ISSUER: &str = "https://auth.openai.com";
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ENROLL_SCOPE: &str = "codex.remote_control.enroll";
const RELAY_SCOPE: &str = "remote_control_controller_websocket";
const ALGORITHM: &str = "ecdsa_p256_sha256";
const PROTECTION_CLASS: &str = "os_protected_nonextractable";
const ORIGINATOR: &str = "Codex Desktop";
pub type RelaySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct PairRelay {
    pub socket: RelaySocket,
    pub client_id: String,
    pub env_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairConnectRequest {
    pub code: String,
    pub kind: PairCodeKind,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairCodeKind {
    Manual,
    Qr,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PairingProgress {
    attempt_id: String,
    stage: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_code: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PairTiming {
    attempt_id: String,
    phase: &'static str,
    elapsed_ms: f64,
    build_profile: &'static str,
}

#[derive(Deserialize, Clone)]
struct AuthContext {
    access_token: String,
    account_id: String,
    account_user_id: String,
}

trait DeviceIdentity {
    fn create(app: &AppHandle, key_container: &str) -> Result<String, String>;
    fn sign(app: &AppHandle, key_container: &str, payload: &[u8]) -> Result<Vec<u8>, String>;
    fn delete(app: &AppHandle, key_container: &str) -> Result<(), String>;
}

struct PlatformDeviceIdentity;

#[cfg(windows)]
impl DeviceIdentity for PlatformDeviceIdentity {
    fn create(_app: &AppHandle, key_container: &str) -> Result<String, String> {
        create_windows_device_key(key_container)
    }

    fn sign(_app: &AppHandle, key_container: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        sign_with_windows_device_key(key_container, payload)
    }

    fn delete(_app: &AppHandle, key_container: &str) -> Result<(), String> {
        delete_windows_device_key(key_container)
    }
}

#[cfg(target_os = "android")]
impl DeviceIdentity for PlatformDeviceIdentity {
    fn create(app: &AppHandle, key_container: &str) -> Result<String, String> {
        crate::android_security::create_device_identity(app, key_container)
    }

    fn sign(app: &AppHandle, key_container: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
        crate::android_security::sign(app, key_container, payload)
    }

    fn delete(app: &AppHandle, key_container: &str) -> Result<(), String> {
        crate::android_security::delete_device_identity(app, key_container)
    }
}

#[cfg(not(any(windows, target_os = "android")))]
impl DeviceIdentity for PlatformDeviceIdentity {
    fn create(_app: &AppHandle, _key_container: &str) -> Result<String, String> {
        Err("公式Pair用のOS保護端末鍵をこのプラットフォームでは利用できません".to_string())
    }

    fn sign(_app: &AppHandle, _key_container: &str, _payload: &[u8]) -> Result<Vec<u8>, String> {
        Err("公式Pair用のOS保護端末鍵をこのプラットフォームでは利用できません".to_string())
    }

    fn delete(_app: &AppHandle, _key_container: &str) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Enrollment {
    account_user_id: String,
    client_id: String,
    key_id: String,
    key_container: String,
    algorithm: String,
    protection_class: String,
    public_key_spki_der_base64: String,
}

#[derive(Deserialize)]
struct EnrollmentStart {
    account_user_id: String,
    client_id: String,
    device_key_challenge: EnrollmentChallenge,
}

#[derive(Deserialize)]
struct EnrollmentChallenge {
    challenge_token: String,
    nonce: String,
    challenge_id: String,
    challenge_expires_at: Value,
    purpose: String,
    audience: String,
    target_origin: String,
    target_path: String,
    account_user_id: String,
    client_id: String,
    device_identity_hash: Option<String>,
}

#[derive(Deserialize)]
struct RemoteToken {
    account_user_id: String,
    client_id: String,
    remote_control_token: String,
    expires_at: String,
    scopes: Vec<String>,
}

#[derive(Deserialize)]
struct EnvironmentPage {
    items: Vec<Environment>,
    cursor: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Environment {
    env_id: String,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebsocketChallenge {
    #[serde(rename = "type")]
    kind: String,
    nonce: String,
    purpose: String,
    audience: String,
    session_id: String,
    target_origin: String,
    target_path: String,
    account_user_id: String,
    client_id: String,
    token_sha256_base64url: String,
    token_expires_at: i64,
    scopes: Vec<String>,
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
}

pub async fn connect_with_pair(
    app: &AppHandle,
    client: &Client,
    request: PairConnectRequest,
    attempt_id: &str,
    started: Instant,
    mut cancelled: watch::Receiver<bool>,
) -> Result<PairRelay, String> {
    progress(
        app,
        attempt_id,
        "auth",
        "Codex のChatGPT認証を確認しています",
    );
    let material = auth_broker::authenticate(app, client, attempt_id, cancelled.clone()).await?;
    let auth = auth_context_from_token(material.access_token)?;
    timing(app, attempt_id, "auth_loaded", started);

    if *cancelled.borrow() {
        return Err("Pair接続をキャンセルしました".to_string());
    }

    let remaining = async {
        progress(app, attempt_id, "device", "このPCの端末鍵を確認しています");
        let (enrollment, token) = authorize_client(app, client, &auth, attempt_id).await?;
        timing(app, attempt_id, "enrollment_authorized", started);

        let before = list_environments(client, &auth, &enrollment.client_id).await?;
        timing(app, attempt_id, "environments_before_pair", started);
        progress(app, attempt_id, "pair", "Pairコードを確認しています");
        let pair_response = claim_pair_code(client, &auth, &enrollment.client_id, &request).await?;
        timing(app, attempt_id, "pair_authenticated", started);

        progress(
            app,
            attempt_id,
            "environment",
            "接続先のCodexを待っています",
        );
        let env_id = choose_environment(
            client,
            &auth,
            &enrollment.client_id,
            &before,
            &pair_response,
        )
        .await?;
        timing(app, attempt_id, "environment_selected", started);

        progress(app, attempt_id, "relay", "公式リレーへ接続しています");
        let socket = open_relay(app, &auth, &enrollment, &token).await?;
        timing(app, attempt_id, "relay_connected", started);
        Ok(PairRelay {
            socket,
            client_id: enrollment.client_id,
            env_id,
        })
    };
    tokio::select! {
        changed = cancelled.changed() => {
            let _ = changed;
            Err("Pair接続をキャンセルしました".to_string())
        }
        result = remaining => result,
    }
}

pub async fn prepare_for_pairing(
    app: &AppHandle,
    client: &Client,
    attempt_id: &str,
    started: Instant,
    mut cancelled: watch::Receiver<bool>,
) -> Result<(), String> {
    progress(
        app,
        attempt_id,
        "auth",
        "Codex のChatGPT認証を確認しています",
    );
    let material = auth_broker::authenticate(app, client, attempt_id, cancelled.clone()).await?;
    let auth = auth_context_from_token(material.access_token)?;
    timing(app, attempt_id, "auth_loaded", started);

    if *cancelled.borrow() {
        return Err("Pair接続の準備をキャンセルしました".to_string());
    }

    let preparation = async {
        progress(
            app,
            attempt_id,
            "device",
            "この端末の端末鍵を確認しています",
        );
        let _ = authorize_client(app, client, &auth, attempt_id).await?;
        timing(app, attempt_id, "enrollment_authorized", started);
        progress(
            app,
            attempt_id,
            "authorize",
            "準備が完了しました。接続先で新しいPairコードを発行してください",
        );
        Ok(())
    };
    tokio::select! {
        changed = cancelled.changed() => {
            let _ = changed;
            Err("Pair接続の準備をキャンセルしました".to_string())
        }
        result = preparation => result,
    }
}

fn progress(app: &AppHandle, attempt_id: &str, stage: &'static str, detail: &str) {
    let _ = app.emit(
        "pairing-progress",
        PairingProgress {
            attempt_id: attempt_id.to_string(),
            stage,
            detail: detail.to_string(),
            verification_url: None,
            user_code: None,
        },
    );
}

fn timing(app: &AppHandle, attempt_id: &str, phase: &'static str, started: Instant) {
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let event = PairTiming {
        attempt_id: attempt_id.to_string(),
        phase,
        elapsed_ms,
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    };
    eprintln!(
        "connection_timing build={} phase={} elapsed_ms={elapsed_ms:.2}",
        event.build_profile, phase
    );
    let _ = app.emit("connection-timing", event);
}

fn auth_context_from_token(access_token: String) -> Result<AuthContext, String> {
    let claims = jwt_claims(&access_token)?;
    let expires_at = claims
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or_else(|| "Codexのアクセストークンに有効期限がありません".to_string())?;
    if expires_at <= now_unix().saturating_add(60) {
        return Err("CodexのChatGPT認証が期限切れです".to_string());
    }
    let account_user_id = claims
        .get("https://api.openai.com/auth")
        .and_then(|value| {
            value
                .get("chatgpt_account_user_id")
                .or_else(|| value.get("account_user_id"))
        })
        .and_then(Value::as_str)
        .ok_or_else(|| "ChatGPTアカウントのユーザーIDを取得できません".to_string())?;
    let account_id = claims
        .get("https://api.openai.com/auth")
        .and_then(|value| {
            value
                .get("chatgpt_account_id")
                .or_else(|| value.get("account_id"))
        })
        .and_then(Value::as_str)
        .ok_or_else(|| "ChatGPTアカウントIDを取得できません".to_string())?;
    Ok(AuthContext {
        access_token,
        account_id: account_id.to_string(),
        account_user_id: account_user_id.to_string(),
    })
}

fn jwt_claims(token: &str) -> Result<Value, String> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| "Codexのアクセストークンが不正です".to_string())?;
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "Codexのアクセストークンを解析できません".to_string())?;
    serde_json::from_slice(&decoded)
        .map_err(|_| "Codexのアクセストークンを解析できません".to_string())
}

async fn authorize_client(
    app: &AppHandle,
    client: &Client,
    auth: &AuthContext,
    attempt_id: &str,
) -> Result<(Enrollment, RemoteToken), String> {
    let path = enrollment_path(app)?;
    if let Some(enrollment) = read_enrollment(&path, &auth.account_user_id) {
        if PlatformDeviceIdentity::sign(app, &enrollment.key_container, b"hutoncodex-key-check")
            .is_ok()
        {
            match refresh_enrollment(app, client, auth, &enrollment).await {
                Ok(token) => return Ok((enrollment, token)),
                Err(error)
                    if error.contains("404")
                        || error.contains("not found")
                        || is_local_device_key_error(&error) =>
                {
                    discard_enrollment(app, &path, &enrollment)?;
                }
                Err(error) => return Err(error),
            }
        } else {
            discard_enrollment(app, &path, &enrollment)?;
        }
    }

    progress(
        app,
        attempt_id,
        "authorize",
        "ブラウザで端末登録を承認してください",
    );
    let start: EnrollmentStart = post_json(
        client,
        auth,
        "/codex/remote/control/client/enroll/start",
        &json!({}),
    )
    .await?;
    if !account_user_ids_match(&start.account_user_id, &auth.account_user_id) {
        return Err("端末登録のChatGPTアカウントが一致しません".to_string());
    }

    let key_id = Uuid::new_v4().to_string();
    let key_container = format!("OpenAI Codex Remote {key_id}");
    let public_key_spki_der_base64 = PlatformDeviceIdentity::create(app, &key_container)?;
    let enrollment = Enrollment {
        account_user_id: start.account_user_id,
        client_id: start.client_id,
        key_id,
        key_container,
        algorithm: ALGORITHM.to_string(),
        protection_class: PROTECTION_CLASS.to_string(),
        public_key_spki_der_base64,
    };

    let enrollment_result: Result<RemoteToken, String> = async {
        let step_up_token = oauth_step_up(app, client, auth).await?;
        validate_step_up_token(&step_up_token, &auth.account_user_id)?;
        let proof = sign_enrollment_challenge(
            app,
            &start.device_key_challenge,
            &enrollment,
            "/backend-api/codex/remote/control/client/enroll/finish",
            false,
        )?;
        let body = json!({
            "client_id": enrollment.client_id,
            "step_up_token": step_up_token,
            "device_identity": device_identity(&enrollment),
            "device_key_proof": proof,
        });
        let result: RemoteToken = post_json(
            client,
            auth,
            "/codex/remote/control/client/enroll/finish",
            &body,
        )
        .await?;
        validate_remote_token(&result, &enrollment)?;
        write_enrollment(&path, &enrollment)?;
        Ok(result)
    }
    .await;
    match enrollment_result {
        Ok(result) => Ok((enrollment, result)),
        Err(error) => {
            let _ = PlatformDeviceIdentity::delete(app, &enrollment.key_container);
            Err(error)
        }
    }
}

fn is_local_device_key_error(error: &str) -> bool {
    error.contains("Windows端末鍵")
        || error.contains("Android端末鍵")
        || error.contains("OS保護端末鍵")
}

fn discard_enrollment(
    app: &AppHandle,
    path: &PathBuf,
    enrollment: &Enrollment,
) -> Result<(), String> {
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("使用できない端末登録情報を削除できません: {error}"))?;
    }
    let _ = PlatformDeviceIdentity::delete(app, &enrollment.key_container);
    Ok(())
}

async fn refresh_enrollment(
    app: &AppHandle,
    client: &Client,
    auth: &AuthContext,
    enrollment: &Enrollment,
) -> Result<RemoteToken, String> {
    let start: EnrollmentStart = post_json(
        client,
        auth,
        "/codex/remote/control/client/refresh/start",
        &json!({ "client_id": enrollment.client_id }),
    )
    .await?;
    if start.client_id != enrollment.client_id
        || !account_user_ids_match(&start.account_user_id, &enrollment.account_user_id)
    {
        return Err("保存済み端末と更新要求が一致しません".to_string());
    }
    let proof = sign_enrollment_challenge(
        app,
        &start.device_key_challenge,
        enrollment,
        "/backend-api/codex/remote/control/client/refresh/finish",
        true,
    )?;
    let token: RemoteToken = post_json(
        client,
        auth,
        "/codex/remote/control/client/refresh/finish",
        &json!({ "client_id": enrollment.client_id, "device_key_proof": proof }),
    )
    .await?;
    validate_remote_token(&token, enrollment)?;
    Ok(token)
}

fn enrollment_path(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("アプリデータの保存先を取得できません: {error}"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("アプリデータの保存先を作成できません: {error}"))?;
    Ok(directory.join("remote-control-enrollment.json"))
}

fn read_enrollment(path: &PathBuf, account_user_id: &str) -> Option<Enrollment> {
    let enrollment: Enrollment = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    account_user_ids_match(&enrollment.account_user_id, account_user_id).then_some(enrollment)
}

fn account_user_ids_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left_user = left.split_once("__").map_or(left, |(user, _)| user);
    let right_user = right.split_once("__").map_or(right, |(user, _)| user);
    !left_user.is_empty() && left_user == right_user
}

fn write_enrollment(path: &PathBuf, enrollment: &Enrollment) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(enrollment)
        .map_err(|error| format!("端末登録情報を保存できません: {error}"))?;
    fs::write(path, encoded).map_err(|error| format!("端末登録情報を保存できません: {error}"))
}

fn device_identity(enrollment: &Enrollment) -> Value {
    json!({
        "key_id": enrollment.key_id,
        "public_key_spki_der_base64": enrollment.public_key_spki_der_base64,
        "algorithm": enrollment.algorithm,
        "protection_class": enrollment.protection_class,
    })
}

fn device_identity_hash(enrollment: &Enrollment) -> String {
    let identity = json!({
        "algorithm": enrollment.algorithm,
        "keyId": enrollment.key_id,
        "protectionClass": enrollment.protection_class,
        "publicKeySpkiDerBase64": enrollment.public_key_spki_der_base64,
    });
    sha256_base64url(identity.to_string().as_bytes())
}

fn sign_enrollment_challenge(
    app: &AppHandle,
    challenge: &EnrollmentChallenge,
    enrollment: &Enrollment,
    expected_path: &str,
    require_identity_hash: bool,
) -> Result<Value, String> {
    if challenge.purpose != "remote_control_client_enrollment"
        || challenge.audience != "remote_control_client_enrollment"
        || !account_user_ids_match(&challenge.account_user_id, &enrollment.account_user_id)
        || challenge.client_id != enrollment.client_id
        || challenge.target_origin != "https://chatgpt.com"
        || challenge.target_path != expected_path
    {
        return Err("端末登録チャレンジの接続先が一致しません".to_string());
    }
    let identity_hash = device_identity_hash(enrollment);
    if require_identity_hash && challenge.device_identity_hash.as_deref() != Some(&identity_hash) {
        return Err("端末登録チャレンジの端末識別子が一致しません".to_string());
    }
    let signed = json!({
        "accountUserId": challenge.account_user_id,
        "audience": challenge.audience,
        "challengeExpiresAt": challenge.challenge_expires_at,
        "challengeId": challenge.challenge_id,
        "clientId": challenge.client_id,
        "deviceIdentitySha256Base64url": identity_hash,
        "nonce": challenge.nonce,
        "targetOrigin": challenge.target_origin,
        "targetPath": challenge.target_path,
        "type": "remoteControlClientEnrollment",
    });
    device_key_proof(
        app,
        enrollment,
        signed,
        Some(challenge.challenge_token.as_str()),
        false,
    )
}

fn device_key_proof(
    app: &AppHandle,
    enrollment: &Enrollment,
    payload: Value,
    challenge_token: Option<&str>,
    websocket: bool,
) -> Result<Value, String> {
    let signed_payload = json!({
        "domain": "codex-device-key-sign-payload/v1",
        "payload": payload,
    })
    .to_string()
    .into_bytes();
    let signature = PlatformDeviceIdentity::sign(app, &enrollment.key_container, &signed_payload)?;
    let signature = general_purpose::STANDARD.encode(signature);
    let signed_payload = general_purpose::STANDARD.encode(signed_payload);
    if websocket {
        Ok(json!({
            "type": "device_key_proof",
            "keyId": enrollment.key_id,
            "signatureDerBase64": signature,
            "signedPayloadBase64": signed_payload,
            "algorithm": enrollment.algorithm,
        }))
    } else {
        Ok(json!({
            "challenge_token": challenge_token,
            "key_id": enrollment.key_id,
            "signature_der_base64": signature,
            "signed_payload_base64": signed_payload,
            "algorithm": enrollment.algorithm,
        }))
    }
}

async fn oauth_step_up(
    app: &AppHandle,
    client: &Client,
    auth: &AuthContext,
) -> Result<String, String> {
    let (listener, port) = bind_oauth_listener().await?;
    let verifier = random_base64url(48);
    let challenge = sha256_base64url(verifier.as_bytes());
    let state = random_base64url(32);
    let redirect_uri = format!("http://localhost:{port}/auth/callback");
    let mut authorize =
        Url::parse(&format!("{AUTH_ISSUER}/oauth/authorize")).map_err(|error| error.to_string())?;
    authorize
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", ENROLL_SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("originator", ORIGINATOR)
        .append_pair("reauth", "remote_control")
        .append_pair("max_age", "0")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("allowed_workspace_id", &auth.account_id)
        .append_pair("current_workspace_id", &auth.account_id);
    open_browser(app, authorize.as_str())?;
    let code = wait_for_oauth_callback(listener, &state).await?;
    let response = client
        .post(format!("{AUTH_ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", OAUTH_CLIENT_ID),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|error| format!("追加認証を完了できません: {error}"))?;
    parse_response::<OAuthTokenResponse>(response)
        .await
        .map(|token| token.access_token)
}

async fn bind_oauth_listener() -> Result<(TcpListener, u16), String> {
    for port in [1455_u16, 1457_u16] {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return Ok((listener, port));
        }
    }
    Err("追加認証用のローカルポート (1455/1457) を開けません".to_string())
}

async fn wait_for_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    timeout(Duration::from_secs(300), async {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|error| format!("追加認証の応答を受信できません: {error}"))?;
            let target = match timeout(
                Duration::from_secs(10),
                read_http_request_target(&mut stream),
            )
            .await
            {
                Ok(Ok(target)) => target,
                _ => {
                    write_oauth_response(
                        &mut stream,
                        "400 Bad Request",
                        "追加認証の応答を読み取れませんでした。",
                    )
                    .await;
                    continue;
                }
            };
            let Ok(callback) = Url::parse(&format!("http://localhost{target}")) else {
                write_oauth_response(
                    &mut stream,
                    "400 Bad Request",
                    "追加認証の応答URLが不正です。",
                )
                .await;
                continue;
            };
            let values: HashMap<_, _> = callback.query_pairs().into_owned().collect();
            let state_matches = values
                .get("state")
                .is_some_and(|value| value == expected_state);
            let code = values.get("code").filter(|value| !value.is_empty()).cloned();
            if state_matches && callback.path() == "/auth/callback" {
                if let Some(code) = code {
                    write_oauth_response(
                        &mut stream,
                        "200 OK",
                        "hutoncodexの端末登録が完了しました。このタブを閉じてアプリに戻ってください。",
                    )
                    .await;
                    return Ok(code);
                }
            }
            write_oauth_response(
                &mut stream,
                "400 Bad Request",
                "hutoncodexの端末登録を完了できませんでした。正しい認証ページから再試行してください。",
            )
            .await;
        }
    })
    .await
    .map_err(|_| "追加認証がタイムアウトしました".to_string())?
}

async fn read_http_request_target<R: AsyncRead + Unpin>(stream: &mut R) -> Result<String, String> {
    const MAX_REQUEST_LINE_BYTES: usize = 16 * 1024;
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let size = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("追加認証の応答を読み取れません: {error}"))?;
        if size == 0 {
            return Err("追加認証のHTTPリクエストが途中で終了しました".to_string());
        }
        buffer.extend_from_slice(&chunk[..size]);
        if buffer.len() > MAX_REQUEST_LINE_BYTES {
            return Err("追加認証のHTTPリクエストが長すぎます".to_string());
        }
        if buffer.windows(2).any(|window| window == b"\r\n") || buffer.contains(&b'\n') {
            break;
        }
    }
    let request = String::from_utf8_lossy(&buffer);
    let mut parts = request
        .lines()
        .next()
        .ok_or_else(|| "追加認証の応答が不正です".to_string())?
        .split_whitespace();
    let method = parts.next();
    let target = parts.next();
    if method != Some("GET") || !target.is_some_and(|value| value.starts_with('/')) {
        return Err("追加認証の応答が不正です".to_string());
    }
    Ok(target.unwrap_or_default().to_string())
}

async fn write_oauth_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

fn open_browser(app: &AppHandle, url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = app;
        StdCommand::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map_err(|error| format!("追加認証ページを開けません: {error}"))?;
        Ok(())
    }
    #[cfg(target_os = "android")]
    {
        crate::android_security::open_url(app, url)
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = app;
        let _ = url;
        Err("このビルドではブラウザ起動に対応していません".to_string())
    }
}

fn validate_step_up_token(token: &str, account_user_id: &str) -> Result<(), String> {
    let claims = jwt_claims(token)?;
    let token_user_id = claims
        .get("https://api.openai.com/auth")
        .and_then(|value| {
            value
                .get("chatgpt_account_user_id")
                .or_else(|| value.get("account_user_id"))
        })
        .and_then(Value::as_str);
    if !token_user_id
        .is_some_and(|token_user_id| account_user_ids_match(token_user_id, account_user_id))
    {
        return Err("追加認証したChatGPTアカウントが一致しません".to_string());
    }
    let mut scopes: HashSet<&str> = claims
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if let Some(values) = claims.get("scp").and_then(Value::as_array) {
        scopes.extend(values.iter().filter_map(Value::as_str));
    }
    if scopes.len() != 1 || !scopes.contains(ENROLL_SCOPE) {
        return Err("追加認証にRemote Control権限がありません".to_string());
    }
    let issued_at = claims
        .get("iat")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if now_unix().saturating_sub(issued_at) > 300 {
        return Err("追加認証トークンが古すぎます".to_string());
    }
    let password_auth_time = claims
        .get("pwd_auth_time")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let now_ms = now_unix().saturating_mul(1000);
    if now_ms.saturating_sub(password_auth_time) > 300_000 {
        return Err("追加認証でパスワード確認が完了していません".to_string());
    }
    Ok(())
}

async fn claim_pair_code(
    client: &Client,
    auth: &AuthContext,
    client_id: &str,
    request: &PairConnectRequest,
) -> Result<Value, String> {
    let code = extract_pair_code(&request.code, &request.kind)?;
    let body = match request.kind {
        PairCodeKind::Manual => json!({ "client_id": client_id, "manual_pairing_code": code }),
        PairCodeKind::Qr => json!({ "client_id": client_id, "pairing_code": code }),
    };
    post_json_value(client, auth, "/wham/remote/control/client/pair", &body).await
}

fn extract_pair_code(value: &str, kind: &PairCodeKind) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Pairコードを入力してください".to_string());
    }
    if matches!(kind, PairCodeKind::Qr) {
        if let Ok(url) = Url::parse(value) {
            if let Some((_, code)) = url.query_pairs().find(|(key, _)| key == "pairing_code") {
                return validate_pair_code(code.as_ref());
            }
        }
    }
    validate_pair_code(value)
}

fn validate_pair_code(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err("Pairコードの形式が不正です".to_string());
    }
    Ok(value.to_string())
}

async fn choose_environment(
    client: &Client,
    auth: &AuthContext,
    client_id: &str,
    before: &[Environment],
    pair_response: &Value,
) -> Result<String, String> {
    if let Some(env_id) = pair_response
        .get("environment_id")
        .or_else(|| pair_response.get("env_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Ok(env_id.to_string());
    }
    for attempt in 0..15 {
        let environments = list_environments(client, auth, client_id).await?;
        if let Some(environment) = select_paired_environment(before, &environments) {
            return Ok(environment.env_id.clone());
        }
        if attempt < 14 {
            sleep(Duration::from_secs(1)).await;
        }
    }
    Err("Pairは受理されましたが、接続先のCodexがオンラインになりませんでした".to_string())
}

fn select_paired_environment<'a>(
    before: &[Environment],
    current: &'a [Environment],
) -> Option<&'a Environment> {
    let previously_online: HashSet<_> = before
        .iter()
        .filter(|item| item.online)
        .map(|item| item.env_id.as_str())
        .collect();
    current
        .iter()
        .find(|item| item.online && !previously_online.contains(item.env_id.as_str()))
}

async fn list_environments(
    client: &Client,
    auth: &AuthContext,
    client_id: &str,
) -> Result<Vec<Environment>, String> {
    let path = format!(
        "/codex/remote/control/clients/{}/environments?limit=100",
        url::form_urlencoded::byte_serialize(client_id.as_bytes()).collect::<String>()
    );
    let response = client
        .get(format!("{API_BASE}{path}"))
        .headers(auth_headers(auth)?)
        .send()
        .await
        .map_err(|error| format!("接続先一覧を取得できません: {error}"))?;
    let page: EnvironmentPage = parse_response(response).await?;
    let _ = page.cursor;
    let _labels: Vec<_> = page
        .items
        .iter()
        .map(|item| item.display_name.as_ref().or(item.name.as_ref()))
        .collect();
    Ok(page.items)
}

async fn open_relay(
    app: &AppHandle,
    auth: &AuthContext,
    enrollment: &Enrollment,
    token: &RemoteToken,
) -> Result<RelaySocket, String> {
    validate_remote_token(token, enrollment)?;
    let mut request = RELAY_URL
        .into_client_request()
        .map_err(|error| format!("公式リレーの接続要求を作成できません: {error}"))?;
    let headers = request.headers_mut();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", auth.access_token))
            .map_err(|_| "Codexのアクセストークンが不正です".to_string())?,
    );
    insert_header(headers, "chatgpt-account-id", &auth.account_id)?;
    insert_header(headers, "originator", ORIGINATOR)?;
    insert_header(
        headers,
        "x-codex-client-session-token",
        &format!("Bearer {}", token.remote_control_token),
    )?;
    insert_header(headers, "x-codex-client-id", &enrollment.client_id)?;
    insert_header(headers, "x-codex-protocol-version", "3")?;
    let (mut socket, response) = timeout(Duration::from_secs(20), connect_async(request))
        .await
        .map_err(|_| "公式リレーへの接続がタイムアウトしました".to_string())?
        .map_err(|error| format!("公式リレーへ接続できません: {error}"))?;
    if response.status().as_u16() != 101 {
        return Err(format!(
            "公式リレーが接続を拒否しました (HTTP {})",
            response.status()
        ));
    }
    let challenge = timeout(Duration::from_secs(15), socket.next())
        .await
        .map_err(|_| "公式リレーの端末確認がタイムアウトしました".to_string())?
        .ok_or_else(|| "公式リレーが端末確認前に切断されました".to_string())?
        .map_err(|error| format!("公式リレーの端末確認を受信できません: {error}"))?;
    let Message::Text(challenge) = challenge else {
        return Err("公式リレーから不正な端末確認を受信しました".to_string());
    };
    let challenge: WebsocketChallenge = serde_json::from_str(&challenge)
        .map_err(|error| format!("公式リレーの端末確認を解析できません: {error}"))?;
    let proof = sign_websocket_challenge(app, &challenge, enrollment, token)?;
    socket
        .send(Message::Text(proof.to_string().into()))
        .await
        .map_err(|error| format!("公式リレーへ端末証明を送信できません: {error}"))?;
    Ok(socket)
}

fn sign_websocket_challenge(
    app: &AppHandle,
    challenge: &WebsocketChallenge,
    enrollment: &Enrollment,
    token: &RemoteToken,
) -> Result<Value, String> {
    let expires_at = parse_rfc3339_unix(&token.expires_at)?;
    if challenge.kind != "device_key_challenge"
        || challenge.purpose != "remote_control_client_websocket"
        || challenge.audience != "remote_control_client_websocket"
        || !account_user_ids_match(&challenge.account_user_id, &enrollment.account_user_id)
        || challenge.client_id != enrollment.client_id
        || challenge.target_origin != "https://chatgpt.com"
        || challenge.target_path != "/backend-api/codex/remote/control/client"
        || challenge.token_sha256_base64url
            != sha256_base64url(token.remote_control_token.as_bytes())
        || challenge.token_expires_at != expires_at
        || challenge.scopes != [RELAY_SCOPE]
    {
        return Err("公式リレーの端末確認が現在の接続情報と一致しません".to_string());
    }
    let payload = json!({
        "accountUserId": challenge.account_user_id,
        "audience": challenge.audience,
        "clientId": challenge.client_id,
        "nonce": challenge.nonce,
        "scopes": challenge.scopes,
        "sessionId": challenge.session_id,
        "targetOrigin": challenge.target_origin,
        "targetPath": challenge.target_path,
        "tokenExpiresAt": challenge.token_expires_at,
        "tokenSha256Base64url": challenge.token_sha256_base64url,
        "type": "remoteControlClientConnection",
    });
    device_key_proof(app, enrollment, payload, None, true)
}

fn validate_remote_token(token: &RemoteToken, enrollment: &Enrollment) -> Result<(), String> {
    if !account_user_ids_match(&token.account_user_id, &enrollment.account_user_id)
        || token.client_id != enrollment.client_id
    {
        return Err("Remote Controlトークンがこの端末と一致しません".to_string());
    }
    if token.scopes != [RELAY_SCOPE] {
        return Err("Remote Controlトークンの権限が不正です".to_string());
    }
    let expires_at = parse_rfc3339_unix(&token.expires_at)?;
    if expires_at <= now_unix() {
        return Err("Remote Controlトークンの有効期限が切れています".to_string());
    }
    Ok(())
}

fn parse_rfc3339_unix(value: &str) -> Result<i64, String> {
    let parsed = time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| "Remote Controlトークンの有効期限が不正です".to_string())?;
    Ok(parsed.unix_timestamp())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn post_json<T: for<'de> Deserialize<'de>>(
    client: &Client,
    auth: &AuthContext,
    path: &str,
    body: &Value,
) -> Result<T, String> {
    let response = client
        .post(format!("{API_BASE}{path}"))
        .headers(auth_headers(auth)?)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Remote Control APIへ接続できません: {error}"))?;
    parse_response(response).await
}

async fn post_json_value(
    client: &Client,
    auth: &AuthContext,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let response = client
        .post(format!("{API_BASE}{path}"))
        .headers(auth_headers(auth)?)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Remote Control APIへ接続できません: {error}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        if status.as_u16() == 401 {
            return Err(
                "CodexのChatGPT認証が期限切れです。`codex login` を実行してください".to_string(),
            );
        }
        return Err(format!(
            "Remote Control APIが要求を拒否しました (HTTP {}): {}",
            status.as_u16(),
            safe_error_detail(&text)
        ));
    }
    if text.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&text)
            .map_err(|error| format!("Remote Control APIの応答を解析できません: {error}"))
    }
}

async fn parse_response<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            return Err(
                "CodexのChatGPT認証が期限切れです。`codex login` を実行してください".to_string(),
            );
        }
        return Err(format!(
            "Remote Control APIが要求を拒否しました (HTTP {}): {}",
            status.as_u16(),
            safe_error_detail(&detail)
        ));
    }
    response
        .json::<T>()
        .await
        .map_err(|error| format!("Remote Control APIの応答を解析できません: {error}"))
}

fn safe_error_detail(detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        return "詳細なし".to_string();
    }
    serde_json::from_str::<Value>(detail)
        .ok()
        .and_then(|value| {
            value
                .get("detail")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| detail.chars().take(240).collect())
}

fn auth_headers(auth: &AuthContext) -> Result<reqwest::header::HeaderMap, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", auth.access_token))
            .map_err(|_| "Codexのアクセストークンが不正です".to_string())?,
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("chatgpt-account-id"),
        reqwest::header::HeaderValue::from_str(&auth.account_id)
            .map_err(|_| "ChatGPTアカウントIDが不正です".to_string())?,
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("originator"),
        reqwest::header::HeaderValue::from_static(ORIGINATOR),
    );
    Ok(headers)
}

fn insert_header(
    headers: &mut http::HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), String> {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_str(value).map_err(|_| format!("接続ヘッダー {name} が不正です"))?,
    );
    Ok(())
}

fn random_base64url(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    general_purpose::URL_SAFE_NO_PAD.encode(value)
}

fn sha256_base64url(value: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value))
}

#[cfg(windows)]
fn create_windows_device_key(container: &str) -> Result<String, String> {
    use windows::{
        core::PCWSTR,
        Win32::Security::Cryptography::{
            NCryptCreatePersistedKey, NCryptExportKey, NCryptFinalizeKey, NCryptFreeObject,
            NCryptOpenStorageProvider, BCRYPT_ECCPUBLIC_BLOB, CERT_KEY_SPEC,
            MS_KEY_STORAGE_PROVIDER, NCRYPT_ECDSA_P256_ALGORITHM, NCRYPT_FLAGS, NCRYPT_HANDLE,
            NCRYPT_KEY_HANDLE, NCRYPT_OVERWRITE_KEY_FLAG, NCRYPT_PROV_HANDLE,
        },
    };
    let wide: Vec<u16> = container.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut provider = NCRYPT_PROV_HANDLE::default();
        NCryptOpenStorageProvider(&mut provider, MS_KEY_STORAGE_PROVIDER, 0)
            .map_err(|error| format!("Windows端末鍵ストアを開けません: {error}"))?;
        let mut key = NCRYPT_KEY_HANDLE::default();
        let result = (|| {
            NCryptCreatePersistedKey(
                provider,
                &mut key,
                NCRYPT_ECDSA_P256_ALGORITHM,
                PCWSTR(wide.as_ptr()),
                CERT_KEY_SPEC(0),
                NCRYPT_OVERWRITE_KEY_FLAG,
            )
            .map_err(|error| format!("Windows端末鍵を作成できません: {error}"))?;
            NCryptFinalizeKey(key, NCRYPT_FLAGS(0))
                .map_err(|error| format!("Windows端末鍵を確定できません: {error}"))?;
            let mut required = 0_u32;
            NCryptExportKey(
                key,
                None,
                BCRYPT_ECCPUBLIC_BLOB,
                None,
                None,
                &mut required,
                NCRYPT_FLAGS(0),
            )
            .map_err(|error| format!("Windows端末鍵の公開鍵を取得できません: {error}"))?;
            let mut blob = vec![0_u8; required as usize];
            NCryptExportKey(
                key,
                None,
                BCRYPT_ECCPUBLIC_BLOB,
                None,
                Some(&mut blob),
                &mut required,
                NCRYPT_FLAGS(0),
            )
            .map_err(|error| format!("Windows端末鍵の公開鍵を取得できません: {error}"))?;
            ecc_public_blob_to_spki(&blob)
        })();
        if key.0 != 0 {
            let _ = NCryptFreeObject(NCRYPT_HANDLE(key.0));
        }
        let _ = NCryptFreeObject(NCRYPT_HANDLE(provider.0));
        result
    }
}

#[cfg(windows)]
fn sign_with_windows_device_key(container: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
    use windows::{
        core::PCWSTR,
        Win32::Security::Cryptography::{
            NCryptFreeObject, NCryptOpenKey, NCryptOpenStorageProvider, NCryptSignHash,
            CERT_KEY_SPEC, MS_KEY_STORAGE_PROVIDER, NCRYPT_FLAGS, NCRYPT_HANDLE, NCRYPT_KEY_HANDLE,
            NCRYPT_PROV_HANDLE,
        },
    };
    let wide: Vec<u16> = container.encode_utf16().chain(Some(0)).collect();
    let digest = Sha256::digest(payload);
    unsafe {
        let mut provider = NCRYPT_PROV_HANDLE::default();
        NCryptOpenStorageProvider(&mut provider, MS_KEY_STORAGE_PROVIDER, 0)
            .map_err(|error| format!("Windows端末鍵ストアを開けません: {error}"))?;
        let mut key = NCRYPT_KEY_HANDLE::default();
        let result = (|| {
            NCryptOpenKey(
                provider,
                &mut key,
                PCWSTR(wide.as_ptr()),
                CERT_KEY_SPEC(0),
                NCRYPT_FLAGS(0),
            )
            .map_err(|error| format!("Windows端末鍵を開けません: {error}"))?;
            let mut required = 0_u32;
            NCryptSignHash(key, None, &digest, None, &mut required, NCRYPT_FLAGS(0))
                .map_err(|error| format!("Windows端末鍵で署名できません: {error}"))?;
            let mut raw = vec![0_u8; required as usize];
            NCryptSignHash(
                key,
                None,
                &digest,
                Some(&mut raw),
                &mut required,
                NCRYPT_FLAGS(0),
            )
            .map_err(|error| format!("Windows端末鍵で署名できません: {error}"))?;
            raw.truncate(required as usize);
            ecdsa_raw_to_der(&raw)
        })();
        if key.0 != 0 {
            let _ = NCryptFreeObject(NCRYPT_HANDLE(key.0));
        }
        let _ = NCryptFreeObject(NCRYPT_HANDLE(provider.0));
        result
    }
}

#[cfg(windows)]
fn delete_windows_device_key(container: &str) -> Result<(), String> {
    use windows::{
        core::PCWSTR,
        Win32::Security::Cryptography::{
            NCryptDeleteKey, NCryptFreeObject, NCryptOpenKey, NCryptOpenStorageProvider,
            CERT_KEY_SPEC, MS_KEY_STORAGE_PROVIDER, NCRYPT_FLAGS, NCRYPT_HANDLE, NCRYPT_KEY_HANDLE,
            NCRYPT_PROV_HANDLE,
        },
    };
    let wide: Vec<u16> = container.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut provider = NCRYPT_PROV_HANDLE::default();
        NCryptOpenStorageProvider(&mut provider, MS_KEY_STORAGE_PROVIDER, 0)
            .map_err(|error| format!("Windows端末鍵ストアを開けません: {error}"))?;
        let mut key = NCRYPT_KEY_HANDLE::default();
        let result = NCryptOpenKey(
            provider,
            &mut key,
            PCWSTR(wide.as_ptr()),
            CERT_KEY_SPEC(0),
            NCRYPT_FLAGS(0),
        )
        .and_then(|_| NCryptDeleteKey(key, 0))
        .map_err(|error| format!("未完了のWindows端末鍵を削除できません: {error}"));
        let _ = NCryptFreeObject(NCRYPT_HANDLE(provider.0));
        result
    }
}

#[cfg(windows)]
fn ecc_public_blob_to_spki(blob: &[u8]) -> Result<String, String> {
    if blob.len() < 72 {
        return Err("Windows端末鍵の公開鍵形式が不正です".to_string());
    }
    let size = u32::from_le_bytes(blob[4..8].try_into().unwrap()) as usize;
    if size != 32 || blob.len() < 8 + size * 2 {
        return Err("Windows端末鍵がP-256形式ではありません".to_string());
    }
    let mut spki = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
    ];
    spki.extend_from_slice(&blob[8..8 + size * 2]);
    Ok(general_purpose::STANDARD.encode(spki))
}

#[cfg(any(windows, test))]
fn ecdsa_raw_to_der(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() != 64 {
        return Err("Windows端末鍵の署名形式が不正です".to_string());
    }
    fn integer(bytes: &[u8]) -> Vec<u8> {
        let first = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len() - 1);
        let value = &bytes[first..];
        let needs_zero = value.first().is_some_and(|byte| byte & 0x80 != 0);
        let mut encoded = Vec::with_capacity(value.len() + 3);
        encoded.push(0x02);
        encoded.push((value.len() + usize::from(needs_zero)) as u8);
        if needs_zero {
            encoded.push(0);
        }
        encoded.extend_from_slice(value);
        encoded
    }
    let r = integer(&raw[..32]);
    let s = integer(&raw[32..]);
    let mut der = Vec::with_capacity(r.len() + s.len() + 2);
    der.push(0x30);
    der.push((r.len() + s.len()) as u8);
    der.extend(r);
    der.extend(s);
    Ok(der)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn extracts_qr_pairing_code() {
        let value = extract_pair_code(
            "https://chatgpt.com/codex/pair?pairing_code=opaque-value",
            &PairCodeKind::Qr,
        )
        .unwrap();
        assert_eq!(value, "opaque-value");
    }

    #[test]
    fn preserves_manual_pairing_code_exactly() {
        let value = extract_pair_code("  AbCd-1234-xYz  ", &PairCodeKind::Manual).unwrap();

        assert_eq!(value, "AbCd-1234-xYz");
    }

    #[test]
    fn encodes_raw_p256_signature_as_der() {
        let raw = [1_u8; 64];
        let der = ecdsa_raw_to_der(&raw).unwrap();
        assert_eq!(der[0], 0x30);
        assert_eq!(der[2], 0x02);
    }

    #[test]
    fn selects_only_an_environment_that_became_online_after_pairing() {
        let before = vec![
            Environment {
                env_id: "already-online".to_string(),
                online: true,
                display_name: None,
                name: None,
            },
            Environment {
                env_id: "paired".to_string(),
                online: false,
                display_name: None,
                name: None,
            },
        ];
        let current = vec![
            Environment {
                env_id: "already-online".to_string(),
                online: true,
                display_name: None,
                name: None,
            },
            Environment {
                env_id: "paired".to_string(),
                online: true,
                display_name: None,
                name: None,
            },
        ];
        assert_eq!(
            select_paired_environment(&before, &current).map(|item| item.env_id.as_str()),
            Some("paired")
        );
    }

    #[test]
    fn does_not_fall_back_to_an_existing_online_environment() {
        let before = vec![Environment {
            env_id: "already-online".to_string(),
            online: true,
            display_name: None,
            name: None,
        }];
        let current = before.clone();
        assert!(select_paired_environment(&before, &current).is_none());
    }

    #[test]
    fn recognizes_a_missing_local_device_key_as_stale_enrollment() {
        assert!(is_local_device_key_error(
            "Windows端末鍵を開けません: key does not exist"
        ));
        assert!(is_local_device_key_error("Android端末鍵が見つかりません"));
        assert!(!is_local_device_key_error(
            "Remote Control APIが要求を拒否しました (HTTP 500)"
        ));
    }

    #[test]
    fn accepts_qualified_and_unqualified_ids_for_the_same_account_user() {
        assert!(account_user_ids_match("user-123", "user-123__account-456"));
        assert!(account_user_ids_match("user-123__account-456", "user-123"));
        assert!(!account_user_ids_match(
            "user-123__account-456",
            "user-999__account-456"
        ));
    }

    #[tokio::test]
    async fn reads_an_oauth_request_line_split_across_reads() {
        let (mut reader, mut writer) = duplex(256);
        let read = read_http_request_target(&mut reader);
        let write = async {
            writer.write_all(b"GET /auth/").await.unwrap();
            tokio::task::yield_now().await;
            writer
                .write_all(b"callback?code=abc&state=expected HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
        };
        let (target, _) = tokio::join!(read, write);
        assert_eq!(target.unwrap(), "/auth/callback?code=abc&state=expected");
    }
}
