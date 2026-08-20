use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    AppHandle, Manager, Wry,
};

#[derive(Clone)]
struct AndroidSecurity(PluginHandle<Wry>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AliasRequest<'a> {
    alias: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignRequest<'a> {
    alias: &'a str,
    payload_base64: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthPayloadRequest<'a> {
    payload: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenUrlRequest<'a> {
    url: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipboardRequest<'a> {
    text: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupportedResponse {
    supported: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicKeyResponse {
    public_key_spki_der_base64: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureResponse {
    signature_der_base64: String,
}

#[derive(Deserialize)]
struct AuthPayloadResponse {
    payload: Option<String>,
}

pub fn init() -> TauriPlugin<Wry> {
    Builder::new("android-security")
        .setup(|app, api| {
            let handle =
                api.register_android_plugin("com.codexremote.desktop", "AndroidSecurityPlugin")?;
            app.manage(AndroidSecurity(handle));
            Ok(())
        })
        .build()
}

fn plugin(app: &AppHandle) -> Result<AndroidSecurity, String> {
    app.try_state::<AndroidSecurity>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "Androidの安全な端末鍵サービスを初期化できません".to_string())
}

pub fn is_supported(app: &AppHandle) -> bool {
    plugin(app)
        .and_then(|plugin| {
            plugin
                .0
                .run_mobile_plugin::<SupportedResponse>("isSupported", ())
                .map_err(|_| "Android KeyStoreを確認できません".to_string())
        })
        .is_ok_and(|response| response.supported)
}

pub fn create_device_identity(app: &AppHandle, alias: &str) -> Result<String, String> {
    let response = plugin(app)?
        .0
        .run_mobile_plugin::<PublicKeyResponse>("createDeviceIdentity", AliasRequest { alias })
        .map_err(map_key_error)?;
    general_purpose::STANDARD
        .decode(&response.public_key_spki_der_base64)
        .map_err(|_| "Android端末鍵の公開鍵が不正です".to_string())?;
    Ok(response.public_key_spki_der_base64)
}

pub fn sign(app: &AppHandle, alias: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
    let response = plugin(app)?
        .0
        .run_mobile_plugin::<SignatureResponse>(
            "sign",
            SignRequest {
                alias,
                payload_base64: general_purpose::STANDARD.encode(payload),
            },
        )
        .map_err(map_key_error)?;
    general_purpose::STANDARD
        .decode(response.signature_der_base64)
        .map_err(|_| "Android端末鍵の署名形式が不正です".to_string())
}

pub fn delete_device_identity(app: &AppHandle, alias: &str) -> Result<(), String> {
    plugin(app)?
        .0
        .run_mobile_plugin::<()>("deleteDeviceIdentity", AliasRequest { alias })
        .map_err(map_key_error)
}

pub fn load_auth(app: &AppHandle) -> Result<Option<String>, String> {
    plugin(app)?
        .0
        .run_mobile_plugin::<AuthPayloadResponse>("loadAuth", ())
        .map(|response| response.payload)
        .map_err(|_| "Androidの安全な認証情報を読み取れません".to_string())
}

pub fn store_auth(app: &AppHandle, payload: &str) -> Result<(), String> {
    plugin(app)?
        .0
        .run_mobile_plugin::<()>("storeAuth", AuthPayloadRequest { payload })
        .map_err(|_| "Androidの安全な認証情報を保存できません".to_string())
}

pub fn clear_auth(app: &AppHandle) -> Result<(), String> {
    plugin(app)?
        .0
        .run_mobile_plugin::<()>("clearAuth", ())
        .map_err(|_| "Androidの認証情報を削除できません".to_string())
}

pub fn open_url(app: &AppHandle, url: &str) -> Result<(), String> {
    plugin(app)?
        .0
        .run_mobile_plugin::<()>("openUrl", OpenUrlRequest { url })
        .map_err(|_| "Androidで認証ページを開けません".to_string())
}

pub fn copy_to_clipboard(app: &AppHandle, text: &str) -> Result<(), String> {
    plugin(app)?
        .0
        .run_mobile_plugin::<()>("copyToClipboard", ClipboardRequest { text })
        .map_err(|_| "Androidでデバイスコードをコピーできません".to_string())
}

fn map_key_error(error: impl ToString) -> String {
    let detail = error.to_string();
    if detail.contains("ANDROID_KEY_MISSING") {
        "Android端末鍵が見つかりません".to_string()
    } else if detail.contains("ANDROID_KEY_INVALIDATED") {
        "Android端末鍵が無効になっています".to_string()
    } else {
        "AndroidのOS保護端末鍵を使用できません".to_string()
    }
}
