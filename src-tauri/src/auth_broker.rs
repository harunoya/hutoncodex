#[cfg(any(windows, target_os = "android", test))]
use serde_json::{json, Value};
use tauri::AppHandle;
#[cfg(any(windows, target_os = "android"))]
use tauri::Emitter;
#[cfg(windows)]
use tokio::io::BufReader;
use tokio::sync::{watch, Mutex};
#[cfg(any(windows, test))]
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, Lines},
    time::{timeout, Duration},
};

#[cfg(target_os = "android")]
use {
    crate::android_security,
    base64::{engine::general_purpose, Engine as _},
    reqwest::{Client, StatusCode},
    serde::{Deserialize, Serialize},
    sha2::{Digest, Sha256},
    std::time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
    tokio::time::sleep,
};

#[cfg(any(windows, test))]
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(any(windows, test))]
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
#[cfg(any(windows, test))]
const CANCEL_TIMEOUT: Duration = Duration::from_secs(3);
static AUTH_BROKER_LOCK: Mutex<()> = Mutex::const_new(());

#[cfg(target_os = "android")]
const AUTH_ISSUER: &str = "https://auth.openai.com";
#[cfg(target_os = "android")]
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
#[cfg(target_os = "android")]
const DEVICE_AUTH_TIMEOUT: StdDuration = StdDuration::from_secs(15 * 60);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug)]
pub(crate) struct AuthMaterial {
    pub(crate) access_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(windows, target_os = "android", test))]
pub(crate) struct DeviceCodePrompt {
    pub(crate) verification_url: String,
    pub(crate) user_code: String,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg(any(windows, target_os = "android"))]
struct PairingProgress {
    attempt_id: String,
    stage: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_code: Option<String>,
}

pub(crate) async fn authenticate(
    app: &AppHandle,
    client: &reqwest::Client,
    attempt_id: &str,
    mut cancelled: watch::Receiver<bool>,
) -> Result<AuthMaterial, String> {
    let _guard = tokio::select! {
        guard = AUTH_BROKER_LOCK.lock() => guard,
        changed = cancelled.changed() => {
            let _ = changed;
            return Err("Codexの認証をキャンセルしました".to_string());
        }
    };
    if *cancelled.borrow() {
        return Err("Codexの認証をキャンセルしました".to_string());
    }
    authenticate_platform(app, client, attempt_id, cancelled).await
}

#[cfg(windows)]
async fn authenticate_platform(
    app: &AppHandle,
    _client: &reqwest::Client,
    attempt_id: &str,
    cancelled: watch::Receiver<bool>,
) -> Result<AuthMaterial, String> {
    use std::process::Stdio;
    use tokio::{io::AsyncReadExt, process::Command};

    let mut command = Command::new("codex");
    command
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|error| {
        format!(
            "Codex App Serverを起動できません。Codex CLIのインストールを確認してください: {error}"
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex App Serverへ認証要求を送信できません".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex App Serverの認証応答を読み取れません".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Codex App Serverのエラー出力を取得できません".to_string())?;
    let stderr_task = tokio::spawn(async move {
        let mut sink = Vec::new();
        let _ = stderr.read_to_end(&mut sink).await;
    });

    let mut broker = CodexAuthBroker::new(BufReader::new(stdout), stdin);
    let result = broker
        .authenticate(cancelled, |prompt| {
            emit_device_code_prompt(app, attempt_id, &prompt, false);
            let _ = open_browser(app, &prompt.verification_url);
        })
        .await;

    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(1), child.wait()).await;
    stderr_task.abort();
    result
}

#[cfg(target_os = "android")]
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredAuth {
    access_token: String,
    refresh_token: String,
    id_token: String,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct DeviceUserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    #[serde(default)]
    interval: Option<Value>,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct DeviceTokenResponse {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

#[cfg(target_os = "android")]
async fn authenticate_platform(
    app: &AppHandle,
    client: &Client,
    attempt_id: &str,
    mut cancelled: watch::Receiver<bool>,
) -> Result<AuthMaterial, String> {
    if let Some(stored) = load_stored_auth(app)? {
        if access_token_is_valid(&stored.access_token, 90) {
            return Ok(AuthMaterial {
                access_token: stored.access_token,
            });
        }
        match refresh_stored_auth(client, &stored).await? {
            Some(refreshed) => {
                store_auth(app, &refreshed)?;
                return Ok(AuthMaterial {
                    access_token: refreshed.access_token,
                });
            }
            None => {
                android_security::clear_auth(app)?;
            }
        }
    }

    if *cancelled.borrow() {
        return Err("Codexの認証をキャンセルしました".to_string());
    }

    let device_code = request_device_code(client, &mut cancelled).await?;
    let prompt = DeviceCodePrompt {
        verification_url: format!("{AUTH_ISSUER}/codex/device"),
        user_code: device_code.user_code.clone(),
    };
    let code_copied = android_security::copy_to_clipboard(app, &prompt.user_code).is_ok();
    emit_device_code_prompt(app, attempt_id, &prompt, code_copied);
    let _ = open_browser(app, &prompt.verification_url);

    let code = poll_device_token(client, &device_code, &mut cancelled).await?;
    if sha256_base64url(code.code_verifier.as_bytes()) != code.code_challenge {
        return Err("Codexのデバイスコード認証を安全に確認できません".to_string());
    }
    let exchanged = exchange_device_code(client, &code).await?;
    if !access_token_is_valid(&exchanged.access_token, 30) {
        return Err("Codexの認証応答に有効なアクセストークンがありません".to_string());
    }
    store_auth(app, &exchanged)?;
    Ok(AuthMaterial {
        access_token: exchanged.access_token,
    })
}

#[cfg(target_os = "android")]
fn load_stored_auth(app: &AppHandle) -> Result<Option<StoredAuth>, String> {
    let Some(encoded) = android_security::load_auth(app)? else {
        return Ok(None);
    };
    match serde_json::from_str::<StoredAuth>(&encoded) {
        Ok(auth)
            if !auth.access_token.is_empty()
                && !auth.refresh_token.is_empty()
                && !auth.id_token.is_empty() =>
        {
            Ok(Some(auth))
        }
        _ => {
            android_security::clear_auth(app)?;
            Ok(None)
        }
    }
}

#[cfg(target_os = "android")]
fn store_auth(app: &AppHandle, auth: &StoredAuth) -> Result<(), String> {
    let encoded = serde_json::to_string(auth)
        .map_err(|_| "Codexの認証情報を安全に保存できません".to_string())?;
    android_security::store_auth(app, &encoded)
}

#[cfg(target_os = "android")]
async fn request_device_code(
    client: &Client,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<DeviceUserCodeResponse, String> {
    let request = client
        .post(format!("{AUTH_ISSUER}/api/accounts/deviceauth/usercode"))
        .json(&json!({ "client_id": OAUTH_CLIENT_ID }))
        .send();
    let response = tokio::select! {
        changed = cancelled.changed() => {
            let _ = changed;
            return Err("Codexの認証をキャンセルしました".to_string());
        }
        response = request => response.map_err(|error| {
            format!(
                "Codexのデバイスコードを取得できません ({})",
                auth_transport_error_kind(&error)
            )
        })?,
    };
    if !response.status().is_success() {
        return Err(format!(
            "Codexのデバイスコードを取得できません (HTTP {})",
            response.status().as_u16()
        ));
    }
    let response = response
        .json::<DeviceUserCodeResponse>()
        .await
        .map_err(|_| "Codexのデバイスコード応答を解析できません".to_string())?;
    validate_device_code_response(&response)?;
    Ok(response)
}

#[cfg(target_os = "android")]
async fn poll_device_token(
    client: &Client,
    device_code: &DeviceUserCodeResponse,
    cancelled: &mut watch::Receiver<bool>,
) -> Result<DeviceTokenResponse, String> {
    let started = Instant::now();
    let interval = device_code_interval(device_code.interval.as_ref());
    let mut transient_failures = 0_u8;
    loop {
        if *cancelled.borrow() {
            return Err("Codexの認証をキャンセルしました".to_string());
        }
        let request = client
            .post(format!("{AUTH_ISSUER}/api/accounts/deviceauth/token"))
            .json(&json!({
                "device_auth_id": device_code.device_auth_id,
                "user_code": device_code.user_code,
            }))
            .timeout(StdDuration::from_secs(75))
            .send();
        let response_result = tokio::select! {
            changed = cancelled.changed() => {
                let _ = changed;
                return Err("Codexの認証をキャンセルしました".to_string());
            }
            response = request => response,
        };
        let response = match response_result {
            Ok(response) => {
                transient_failures = 0;
                response
            }
            Err(error)
                if (error.is_timeout() || error.is_connect())
                    && transient_failures < 5
                    && started.elapsed() < DEVICE_AUTH_TIMEOUT =>
            {
                transient_failures = transient_failures.saturating_add(1);
                let remaining = DEVICE_AUTH_TIMEOUT.saturating_sub(started.elapsed());
                tokio::select! {
                    changed = cancelled.changed() => {
                        let _ = changed;
                        return Err("Codexの認証をキャンセルしました".to_string());
                    }
                    _ = sleep(interval.min(remaining)) => {}
                }
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "Codexのデバイスコード認証を確認できません ({})",
                    auth_transport_error_kind(&error)
                ));
            }
        };
        if response.status().is_success() {
            let code = response
                .json::<DeviceTokenResponse>()
                .await
                .map_err(|_| "Codexのデバイスコード認証応答を解析できません".to_string())?;
            if code.authorization_code.is_empty()
                || code.code_verifier.len() < 43
                || code.code_challenge.is_empty()
            {
                return Err("Codexのデバイスコード認証応答が不正です".to_string());
            }
            return Ok(code);
        }
        if response.status() != StatusCode::FORBIDDEN && response.status() != StatusCode::NOT_FOUND
        {
            return Err(format!(
                "Codexのデバイスコード認証に失敗しました (HTTP {})",
                response.status().as_u16()
            ));
        }
        if started.elapsed() >= DEVICE_AUTH_TIMEOUT {
            return Err("Codexのデバイスコード認証がタイムアウトしました".to_string());
        }
        let remaining = DEVICE_AUTH_TIMEOUT.saturating_sub(started.elapsed());
        let wait = interval.min(remaining);
        tokio::select! {
            changed = cancelled.changed() => {
                let _ = changed;
                return Err("Codexの認証をキャンセルしました".to_string());
            }
            _ = sleep(wait) => {}
        }
    }
}

#[cfg(target_os = "android")]
async fn exchange_device_code(
    client: &Client,
    code: &DeviceTokenResponse,
) -> Result<StoredAuth, String> {
    let response = client
        .post(format!("{AUTH_ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.authorization_code.as_str()),
            (
                "redirect_uri",
                "https://auth.openai.com/deviceauth/callback",
            ),
            ("client_id", OAUTH_CLIENT_ID),
            ("code_verifier", code.code_verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|error| {
            format!(
                "Codexのデバイスコードをトークンへ交換できません ({})",
                auth_transport_error_kind(&error)
            )
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "Codexのデバイスコードをトークンへ交換できません (HTTP {})",
            response.status().as_u16()
        ));
    }
    let response = response
        .json::<OAuthTokenResponse>()
        .await
        .map_err(|_| "Codexのトークン応答を解析できません".to_string())?;
    Ok(StoredAuth {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "Codexのトークン応答に更新トークンがありません".to_string())?,
        id_token: response
            .id_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "Codexのトークン応答にIDトークンがありません".to_string())?,
    })
}

#[cfg(target_os = "android")]
async fn refresh_stored_auth(
    client: &Client,
    stored: &StoredAuth,
) -> Result<Option<StoredAuth>, String> {
    let response = client
        .post(format!("{AUTH_ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", stored.refresh_token.as_str()),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|error| {
            format!(
                "Codexの認証を更新できません ({})",
                auth_transport_error_kind(&error)
            )
        })?;
    if matches!(
        response.status(),
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
    ) {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "Codexの認証を更新できません (HTTP {})",
            response.status().as_u16()
        ));
    }
    let response = response
        .json::<OAuthTokenResponse>()
        .await
        .map_err(|_| "Codexの認証更新応答を解析できません".to_string())?;
    if !access_token_is_valid(&response.access_token, 30) {
        return Ok(None);
    }
    Ok(Some(StoredAuth {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .unwrap_or_else(|| stored.refresh_token.clone()),
        id_token: response.id_token.unwrap_or_else(|| stored.id_token.clone()),
    }))
}

#[cfg(target_os = "android")]
fn validate_device_code_response(response: &DeviceUserCodeResponse) -> Result<(), String> {
    let safe_code = response.user_code.chars().all(|character| {
        !character.is_control() && (character.is_ascii_alphanumeric() || character == '-')
    });
    if response.device_auth_id.is_empty()
        || response.device_auth_id.len() > 512
        || response.user_code.is_empty()
        || response.user_code.len() > 64
        || !safe_code
    {
        return Err("Codexのデバイスコード応答が不正です".to_string());
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn device_code_interval(value: Option<&Value>) -> StdDuration {
    let seconds = value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        })
        .unwrap_or(5)
        .clamp(1, 30);
    StdDuration::from_secs(seconds)
}

#[cfg(target_os = "android")]
fn access_token_is_valid(token: &str, minimum_validity_seconds: u64) -> bool {
    let Some(payload) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(decoded) = general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(claims) = serde_json::from_slice::<Value>(&decoded) else {
        return false;
    };
    let Some(expires_at) = claims.get("exp").and_then(Value::as_u64) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    expires_at > now.saturating_add(minimum_validity_seconds)
}

#[cfg(target_os = "android")]
fn sha256_base64url(value: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value))
}

#[cfg(target_os = "android")]
fn auth_transport_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "タイムアウト"
    } else if error.is_connect() {
        "接続エラー"
    } else if error.is_request() {
        "要求エラー"
    } else {
        "通信エラー"
    }
}

#[cfg(not(any(windows, target_os = "android")))]
async fn authenticate_platform(
    _app: &AppHandle,
    _client: &reqwest::Client,
    _attempt_id: &str,
    _cancelled: watch::Receiver<bool>,
) -> Result<AuthMaterial, String> {
    Err("このプラットフォームでは公式Pair用のCodex認証を開始できません".to_string())
}

#[cfg(any(windows, target_os = "android"))]
fn emit_device_code_prompt(
    app: &AppHandle,
    attempt_id: &str,
    prompt: &DeviceCodePrompt,
    code_copied: bool,
) {
    let _ = app.emit(
        "pairing-progress",
        PairingProgress {
            attempt_id: attempt_id.to_string(),
            stage: "auth",
            detail: if code_copied {
                "デバイスコードをコピーしました。ブラウザへ貼り付けてください".to_string()
            } else {
                "ブラウザでCodex CLIのデバイスコードを入力してください".to_string()
            },
            verification_url: Some(prompt.verification_url.clone()),
            user_code: Some(prompt.user_code.clone()),
        },
    );
}

#[cfg(windows)]
fn open_browser(_app: &AppHandle, url: &str) -> Result<(), String> {
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map_err(|error| format!("Codex認証ページを開けません: {error}"))?;
    Ok(())
}

#[cfg(target_os = "android")]
fn open_browser(app: &AppHandle, url: &str) -> Result<(), String> {
    android_security::open_url(app, url)
}

#[cfg(any(windows, test))]
struct CodexAuthBroker<R, W> {
    lines: Lines<R>,
    writer: W,
    next_id: u64,
}

#[cfg(any(windows, test))]
impl<R, W> CodexAuthBroker<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    fn new(reader: R, writer: W) -> Self {
        Self {
            lines: reader.lines(),
            writer,
            next_id: 1,
        }
    }

    async fn authenticate<F>(
        &mut self,
        mut cancelled: watch::Receiver<bool>,
        mut on_device_code: F,
    ) -> Result<AuthMaterial, String>
    where
        F: FnMut(DeviceCodePrompt),
    {
        self.initialize().await?;
        let account = self.read_account(true).await?;
        let status = self.read_auth_status(true).await?;
        if let Some(material) = material_from_status(&account, &status)? {
            return Ok(material);
        }

        let login = self
            .request(
                "account/login/start",
                json!({ "type": "chatgptDeviceCode" }),
            )
            .await?;
        if login.get("type").and_then(Value::as_str) != Some("chatgptDeviceCode") {
            return Err("Codex App Serverがデバイスコード認証を開始できませんでした".to_string());
        }
        let login_id = required_string(&login, "loginId", "ログインID")?;
        let prompt = DeviceCodePrompt {
            verification_url: required_string(&login, "verificationUrl", "認証URL")?,
            user_code: required_string(&login, "userCode", "デバイスコード")?,
        };
        on_device_code(prompt);

        let wait_result = timeout(
            LOGIN_TIMEOUT,
            self.wait_for_login(&login_id, &mut cancelled),
        )
        .await;
        match wait_result {
            Ok(result) => result?,
            Err(_) => {
                self.cancel_login(&login_id).await;
                return Err("Codexのデバイスコード認証がタイムアウトしました".to_string());
            }
        }

        let account = self.read_account(true).await?;
        let status = self.read_auth_status(true).await?;
        material_from_status(&account, &status)?.ok_or_else(|| {
            "デバイスコード認証後もChatGPTアクセストークンを取得できません".to_string()
        })
    }

    async fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "codex_remote_auth_broker",
                    "title": "Codex Remote Auth Broker",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": { "experimentalApi": true }
            }),
        )
        .await?;
        self.notify("initialized", json!({})).await
    }

    async fn read_account(&mut self, refresh: bool) -> Result<Value, String> {
        self.request("account/read", json!({ "refreshToken": refresh }))
            .await
    }

    async fn read_auth_status(&mut self, refresh: bool) -> Result<Value, String> {
        self.request(
            "getAuthStatus",
            json!({ "includeToken": true, "refreshToken": refresh }),
        )
        .await
    }

    async fn wait_for_login(
        &mut self,
        login_id: &str,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        loop {
            if *cancelled.borrow() {
                self.cancel_login(login_id).await;
                return Err("Codexのデバイスコード認証をキャンセルしました".to_string());
            }
            tokio::select! {
                changed = cancelled.changed() => {
                    if changed.is_err() || *cancelled.borrow() {
                        self.cancel_login(login_id).await;
                        return Err("Codexのデバイスコード認証をキャンセルしました".to_string());
                    }
                }
                message = self.read_message() => {
                    let message = message?;
                    if message.get("method").and_then(Value::as_str) == Some("account/login/completed") {
                        let params = message.get("params").unwrap_or(&Value::Null);
                        if params.get("loginId").and_then(Value::as_str) != Some(login_id) {
                            continue;
                        }
                        if params.get("success").and_then(Value::as_bool) == Some(true) {
                            return Ok(());
                        }
                        let error = params.get("error").and_then(Value::as_str)
                            .unwrap_or("Codexのデバイスコード認証に失敗しました");
                        return Err(sanitize_error(error));
                    }
                    self.reject_server_request_if_needed(&message).await?;
                }
            }
        }
    }

    async fn cancel_login(&mut self, login_id: &str) {
        let request = self.request("account/login/cancel", json!({ "loginId": login_id }));
        let _ = timeout(CANCEL_TIMEOUT, request).await;
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.send(json!({ "method": method, "id": id, "params": params }))
            .await?;
        timeout(RPC_TIMEOUT, async {
            loop {
                let message = self.read_message().await?;
                if message.get("id").and_then(Value::as_u64) == Some(id)
                    && message.get("method").is_none()
                {
                    if let Some(error) = message.get("error") {
                        let detail = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex App Serverが認証要求を拒否しました");
                        return Err(sanitize_error(detail));
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                self.reject_server_request_if_needed(&message).await?;
            }
        })
        .await
        .map_err(|_| format!("Codex App Serverの {method} がタイムアウトしました"))?
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({ "method": method, "params": params }))
            .await
    }

    async fn reject_server_request_if_needed(&mut self, message: &Value) -> Result<(), String> {
        let Some(id) = message.get("id") else {
            return Ok(());
        };
        if message.get("method").is_none() {
            return Ok(());
        }
        self.send(json!({
            "id": id,
            "error": { "code": -32601, "message": "Auth Brokerでは未対応の要求です" }
        }))
        .await
    }

    async fn send(&mut self, value: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&value)
            .map_err(|error| format!("Codex App Server認証要求を作成できません: {error}"))?;
        encoded.push(b'\n');
        self.writer
            .write_all(&encoded)
            .await
            .map_err(|error| format!("Codex App Serverへ認証要求を送信できません: {error}"))?;
        self.writer
            .flush()
            .await
            .map_err(|error| format!("Codex App Serverへ認証要求を送信できません: {error}"))
    }

    async fn read_message(&mut self) -> Result<Value, String> {
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|error| format!("Codex App Serverの認証応答を読み取れません: {error}"))?
                .ok_or_else(|| "Codex App Serverが認証処理中に終了しました".to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            return serde_json::from_str(&line)
                .map_err(|error| format!("Codex App Serverの認証応答を解析できません: {error}"));
        }
    }
}

#[cfg(any(windows, test))]
fn material_from_status(account: &Value, status: &Value) -> Result<Option<AuthMaterial>, String> {
    let account_type = account
        .get("account")
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str);
    let auth_method = status.get("authMethod").and_then(Value::as_str);
    if matches!(account_type, Some(value) if value != "chatgpt")
        || matches!(auth_method, Some(value) if value != "chatgpt")
    {
        return Err("公式PairにはCodexのChatGPTログインが必要です".to_string());
    }
    Ok(status
        .get("authToken")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(|token| AuthMaterial {
            access_token: token.to_string(),
        }))
}

#[cfg(any(windows, test))]
fn required_string(value: &Value, key: &str, label: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| format!("Codex App Serverの{label}がありません"))
}

#[cfg(any(windows, test))]
fn sanitize_error(value: &str) -> String {
    if value.contains("Bearer ") || value.contains("eyJ") {
        return "Codexの認証に失敗しました。認証状態を確認してください".to_string();
    }
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(240)
        .collect();
    if cleaned.trim().is_empty() {
        "Codexの認証に失敗しました".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{duplex, split, AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn run_fake_server(
        stream: tokio::io::DuplexStream,
        methods: Arc<Mutex<Vec<String>>>,
        wait_for_cancel: bool,
    ) {
        let (reader, mut writer) = split(stream);
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let message: Value = serde_json::from_str(&line).unwrap();
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            methods.lock().unwrap().push(method.to_string());
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let result = match method {
                "initialize" => json!({ "userAgent": "test" }),
                "account/read" => {
                    let logged_in = methods
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|method| method == "account/login/start")
                        && !wait_for_cancel;
                    if logged_in {
                        json!({ "account": { "type": "chatgpt", "email": null, "planType": "plus" }, "requiresOpenaiAuth": true })
                    } else {
                        json!({ "account": null, "requiresOpenaiAuth": true })
                    }
                }
                "getAuthStatus" => {
                    let logged_in = methods
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|method| method == "account/login/start")
                        && !wait_for_cancel;
                    if logged_in {
                        json!({ "authMethod": "chatgpt", "authToken": "header.payload.signature", "requiresOpenaiAuth": true })
                    } else {
                        json!({ "authMethod": null, "authToken": null, "requiresOpenaiAuth": true })
                    }
                }
                "account/login/start" => json!({
                    "type": "chatgptDeviceCode",
                    "loginId": "login-1",
                    "verificationUrl": "https://auth.openai.com/codex/device",
                    "userCode": "ABCD-1234"
                }),
                "account/login/cancel" => json!({}),
                _ => json!({}),
            };
            let response = format!("{}\n", json!({ "id": id, "result": result }));
            writer.write_all(response.as_bytes()).await.unwrap();
            if method == "account/login/start" && !wait_for_cancel {
                let notification = format!(
                    "{}\n",
                    json!({ "method": "account/login/completed", "params": { "loginId": "login-1", "success": true, "error": null } })
                );
                writer.write_all(notification.as_bytes()).await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn managed_device_auth_does_not_read_an_auth_file_or_parse_cli_output() {
        let (client, server) = duplex(16 * 1024);
        let (reader, writer) = split(client);
        let methods = Arc::new(Mutex::new(Vec::new()));
        let server_task = tokio::spawn(run_fake_server(server, Arc::clone(&methods), false));
        let (_cancel_sender, cancel_receiver) = watch::channel(false);
        let mut prompts = Vec::new();
        let mut broker = CodexAuthBroker::new(BufReader::new(reader), writer);

        let result = broker
            .authenticate(cancel_receiver, |prompt| prompts.push(prompt))
            .await
            .unwrap();

        assert_eq!(result.access_token, "header.payload.signature");
        assert_eq!(prompts.len(), 1);
        let called = methods.lock().unwrap().clone();
        assert!(called.contains(&"account/read".to_string()));
        assert!(called.contains(&"getAuthStatus".to_string()));
        assert!(called.contains(&"account/login/start".to_string()));
        server_task.abort();
    }

    #[tokio::test]
    async fn cancellation_uses_account_login_cancel() {
        let (client, server) = duplex(16 * 1024);
        let (reader, writer) = split(client);
        let methods = Arc::new(Mutex::new(Vec::new()));
        let server_task = tokio::spawn(run_fake_server(server, Arc::clone(&methods), true));
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        let mut broker = CodexAuthBroker::new(BufReader::new(reader), writer);
        let methods_for_cancel = Arc::clone(&methods);
        let cancel_task = tokio::spawn(async move {
            loop {
                if methods_for_cancel
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|method| method == "account/login/start")
                {
                    let _ = cancel_sender.send(true);
                    return;
                }
                tokio::task::yield_now().await;
            }
        });

        let result = broker.authenticate(cancel_receiver, |_| {}).await;

        assert!(result.unwrap_err().contains("キャンセル"));
        assert!(methods
            .lock()
            .unwrap()
            .contains(&"account/login/cancel".to_string()));
        cancel_task.abort();
        server_task.abort();
    }
}
