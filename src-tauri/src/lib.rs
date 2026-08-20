#[cfg(target_os = "android")]
mod android_security;
mod auth_broker;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod discord_presence;
#[cfg(any(target_os = "android", target_os = "ios"))]
#[path = "discord_presence_mobile.rs"]
mod discord_presence;
mod pairing;

use base64::{engine::general_purpose, Engine as _};
use futures_util::{SinkExt, StreamExt};
use http::{
    header::{HeaderValue, AUTHORIZATION},
    StatusCode,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    sync::{mpsc, watch},
    time::timeout,
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message},
};
use url::Url;
use uuid::Uuid;

const RELAY_UNSEGMENTED_LIMIT: usize = 100 * 1024;
const RELAY_CHUNK_BYTES: usize = 72 * 1024;
const RELAY_MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
const RELAY_MAX_CHUNK_BYTES: usize = 128 * 1024;
const RELAY_MAX_SEGMENTS: usize = 512;
const RELAY_MAX_ASSEMBLIES: usize = 64;
const RELAY_MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;
const RELAY_ASSEMBLY_TTL: Duration = Duration::from_secs(60);
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DIRECT_MAX_INCOMING_BYTES: usize = 32 * 1024 * 1024;
const PAIR_CONNECT_TIMEOUT: Duration = Duration::from_secs(16 * 60);

fn direct_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(DIRECT_MAX_INCOMING_BYTES))
        .max_frame_size(Some(DIRECT_MAX_INCOMING_BYTES))
}

#[derive(Clone)]
struct LiveConnection {
    id: u64,
    sender: mpsc::UnboundedSender<Message>,
    transport: Transport,
}

#[derive(Clone)]
enum Transport {
    Direct,
    Relay {
        client_id: String,
        env_id: String,
        stream_id: String,
        next_seq_id: Arc<AtomicU64>,
    },
}

struct AppState {
    connections: Arc<Mutex<HashMap<u64, LiveConnection>>>,
    attempts: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    cancelled_attempts: Arc<Mutex<HashSet<String>>>,
    http_client: Client,
    next_id: AtomicU64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            attempts: Arc::new(Mutex::new(HashMap::new())),
            cancelled_attempts: Arc::new(Mutex::new(HashSet::new())),
            http_client: build_http_client(),
            next_id: AtomicU64::new(1),
        }
    }
}

fn build_http_client() -> Client {
    Client::builder()
        .user_agent("hutoncodex/0.1.0")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| Client::new())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConnectionInfo {
    connection_id: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConnectionStatus {
    connection_id: u64,
    state: &'static str,
    detail: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct IncomingEnvelope {
    connection_id: u64,
    message: Value,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConnectionTiming {
    attempt_id: String,
    phase: String,
    elapsed_ms: f64,
    build_profile: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeCapabilities {
    mobile: bool,
    pairing_supported: bool,
    discord_presence_supported: bool,
}

#[tauri::command]
fn runtime_capabilities(app: AppHandle) -> RuntimeCapabilities {
    #[cfg(windows)]
    let pairing_supported = true;
    #[cfg(target_os = "android")]
    let pairing_supported = android_security::is_supported(&app);
    #[cfg(not(any(windows, target_os = "android")))]
    let pairing_supported = false;
    #[cfg(not(target_os = "android"))]
    let _ = app;

    RuntimeCapabilities {
        mobile: cfg!(any(target_os = "android", target_os = "ios")),
        pairing_supported,
        discord_presence_supported: !cfg!(any(target_os = "android", target_os = "ios")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientTiming {
    attempt_id: String,
    phase: String,
    elapsed_ms: f64,
}

fn begin_connection_attempt(
    attempts: &Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    cancelled_attempts: &Arc<Mutex<HashSet<String>>>,
    attempt_id: &str,
) -> Result<watch::Receiver<bool>, String> {
    validate_attempt_id(attempt_id)?;
    let cancelled_before_start = cancelled_attempts
        .lock()
        .map_err(|_| "接続試行のキャンセル状態を読み取れません".to_string())?
        .remove(attempt_id);
    let (sender, receiver) = watch::channel(cancelled_before_start);
    let sender_for_late_cancel = sender.clone();
    let previous = attempts
        .lock()
        .map_err(|_| "接続試行の状態を更新できません".to_string())?
        .insert(attempt_id.to_string(), sender);
    if let Some(previous) = previous {
        let _ = previous.send(true);
    }
    let cancelled_during_registration = cancelled_attempts
        .lock()
        .map_err(|_| "接続試行のキャンセル状態を読み取れません".to_string())?
        .remove(attempt_id);
    if cancelled_during_registration {
        let _ = sender_for_late_cancel.send(true);
    }
    Ok(receiver)
}

fn finish_connection_attempt(
    attempts: &Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    cancelled_attempts: &Arc<Mutex<HashSet<String>>>,
    attempt_id: &str,
) {
    if let Ok(mut attempts) = attempts.lock() {
        attempts.remove(attempt_id);
    }
    if let Ok(mut cancelled) = cancelled_attempts.lock() {
        cancelled.remove(attempt_id);
    }
}

struct ConnectionAttemptGuard {
    attempts: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    cancelled_attempts: Arc<Mutex<HashSet<String>>>,
    attempt_id: String,
}

impl Drop for ConnectionAttemptGuard {
    fn drop(&mut self) {
        finish_connection_attempt(&self.attempts, &self.cancelled_attempts, &self.attempt_id);
    }
}

fn validate_attempt_id(attempt_id: &str) -> Result<(), String> {
    if attempt_id.is_empty()
        || attempt_id.len() > 128
        || !attempt_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("接続試行IDが不正です".to_string());
    }
    Ok(())
}

fn emit_connection_timing(app: &AppHandle, attempt_id: &str, phase: &str, elapsed_ms: f64) {
    let timing = ConnectionTiming {
        attempt_id: attempt_id.to_string(),
        phase: phase.to_string(),
        elapsed_ms,
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    };
    eprintln!(
        "connection_timing build={} phase={} elapsed_ms={:.2}",
        timing.build_profile, timing.phase, timing.elapsed_ms
    );
    let _ = app.emit("connection-timing", timing);
}

#[tauri::command]
async fn connect_app_server(
    app: AppHandle,
    state: State<'_, AppState>,
    attempt_id: String,
    url: String,
    bearer_token: Option<String>,
) -> Result<ConnectionInfo, String> {
    let started = Instant::now();
    emit_connection_timing(&app, &attempt_id, "tauri_command_start", 0.0);
    let endpoint = validate_endpoint(&url)?;

    let mut request = endpoint
        .as_str()
        .into_client_request()
        .map_err(|error| format!("WebSocketリクエストを作成できません: {error}"))?;
    if let Some(token) = bearer_token.filter(|value| !value.trim().is_empty()) {
        let value = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| "Bearer Tokenに使用できない文字が含まれています".to_string())?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    let mut cancelled =
        begin_connection_attempt(&state.attempts, &state.cancelled_attempts, &attempt_id)?;
    let _attempt_guard = ConnectionAttemptGuard {
        attempts: Arc::clone(&state.attempts),
        cancelled_attempts: Arc::clone(&state.cancelled_attempts),
        attempt_id: attempt_id.clone(),
    };

    let connect_result = tokio::select! {
        changed = cancelled.changed() => {
            let _ = changed;
            Err("接続をキャンセルしました".to_string())
        }
        result = timeout(
            DIRECT_CONNECT_TIMEOUT,
            connect_async_with_config(request, Some(direct_websocket_config()), false),
        ) => {
            match result {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(friendly_connect_error(error.to_string())),
                Err(_) => Err("App Serverへの接続がタイムアウトしました".to_string()),
            }
        }
    };
    let (socket, response) = match connect_result {
        Ok(value) => value,
        Err(error) => {
            return Err(error);
        }
    };
    emit_connection_timing(
        &app,
        &attempt_id,
        "websocket_connected",
        started.elapsed().as_secs_f64() * 1000.0,
    );
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(format!(
            "App Serverが接続を拒否しました (HTTP {})",
            response.status()
        ));
    }

    let connection_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = mpsc::unbounded_channel();
    install_connection(
        &state.connections,
        LiveConnection {
            id: connection_id,
            sender,
            transport: Transport::Direct,
        },
    )?;
    spawn_direct_connection(
        app.clone(),
        Arc::clone(&state.connections),
        connection_id,
        socket,
        receiver,
    );
    emit_connected(&app, connection_id)?;
    Ok(ConnectionInfo { connection_id })
}

#[tauri::command]
async fn connect_paired_app_server(
    app: AppHandle,
    state: State<'_, AppState>,
    attempt_id: String,
    request: pairing::PairConnectRequest,
) -> Result<ConnectionInfo, String> {
    let started = Instant::now();
    emit_connection_timing(&app, &attempt_id, "tauri_command_start", 0.0);
    let cancelled =
        begin_connection_attempt(&state.attempts, &state.cancelled_attempts, &attempt_id)?;
    let _attempt_guard = ConnectionAttemptGuard {
        attempts: Arc::clone(&state.attempts),
        cancelled_attempts: Arc::clone(&state.cancelled_attempts),
        attempt_id: attempt_id.clone(),
    };
    let relay_result = match timeout(
        PAIR_CONNECT_TIMEOUT,
        pairing::connect_with_pair(
            &app,
            &state.http_client,
            request,
            &attempt_id,
            started,
            cancelled,
        ),
    )
    .await
    {
        Ok(value) => value,
        Err(_) => Err("Pair接続がタイムアウトしました".to_string()),
    };
    let relay = match relay_result {
        Ok(value) => value,
        Err(error) => {
            return Err(error);
        }
    };
    let connection_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let stream_id = Uuid::new_v4().to_string();
    let next_seq_id = Arc::new(AtomicU64::new(1));
    let (sender, receiver) = mpsc::unbounded_channel();
    install_connection(
        &state.connections,
        LiveConnection {
            id: connection_id,
            sender,
            transport: Transport::Relay {
                client_id: relay.client_id.clone(),
                env_id: relay.env_id.clone(),
                stream_id: stream_id.clone(),
                next_seq_id: Arc::clone(&next_seq_id),
            },
        },
    )?;
    spawn_relay_connection(
        app.clone(),
        Arc::clone(&state.connections),
        connection_id,
        relay,
        stream_id,
        next_seq_id,
        receiver,
    );
    emit_connected(&app, connection_id)?;
    Ok(ConnectionInfo { connection_id })
}

#[tauri::command]
async fn prepare_pair_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    attempt_id: String,
) -> Result<(), String> {
    let started = Instant::now();
    emit_connection_timing(&app, &attempt_id, "tauri_command_start", 0.0);
    let cancelled =
        begin_connection_attempt(&state.attempts, &state.cancelled_attempts, &attempt_id)?;
    let _attempt_guard = ConnectionAttemptGuard {
        attempts: Arc::clone(&state.attempts),
        cancelled_attempts: Arc::clone(&state.cancelled_attempts),
        attempt_id: attempt_id.clone(),
    };
    match timeout(
        PAIR_CONNECT_TIMEOUT,
        pairing::prepare_for_pairing(&app, &state.http_client, &attempt_id, started, cancelled),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err("Pair接続の準備がタイムアウトしました".to_string()),
    }
}

#[tauri::command]
fn cancel_connection_attempt(state: State<'_, AppState>, attempt_id: String) -> Result<(), String> {
    validate_attempt_id(&attempt_id)?;
    if let Some(attempt) = state
        .attempts
        .lock()
        .map_err(|_| "接続試行の状態を読み取れません".to_string())?
        .get(&attempt_id)
        .cloned()
    {
        let _ = attempt.send(true);
    } else {
        state
            .cancelled_attempts
            .lock()
            .map_err(|_| "接続試行のキャンセル状態を更新できません".to_string())?
            .insert(attempt_id);
    }
    Ok(())
}

#[tauri::command]
fn record_connection_timing(app: AppHandle, timing: ClientTiming) -> Result<(), String> {
    validate_attempt_id(&timing.attempt_id)?;
    if !timing.elapsed_ms.is_finite() || !(0.0..=600_000.0).contains(&timing.elapsed_ms) {
        return Err("接続計測値が不正です".to_string());
    }
    const ALLOWED_PHASES: &[&str] = &[
        "initialize_request_completed",
        "initialized_notification_sent",
        "thread_list_completed",
        "model_list_completed",
        "catalogs_completed",
        "usage_completed",
        "ui_operable",
    ];
    if !ALLOWED_PHASES.contains(&timing.phase.as_str()) {
        return Err("接続計測フェーズが不正です".to_string());
    }
    emit_connection_timing(&app, &timing.attempt_id, &timing.phase, timing.elapsed_ms);
    Ok(())
}

#[tauri::command]
fn discord_presence_update(
    service: State<'_, discord_presence::DiscordPresenceService>,
    update: discord_presence::PresenceUpdate,
) {
    service.update(update);
}

#[tauri::command]
fn discord_presence_get_settings(
    service: State<'_, discord_presence::DiscordPresenceService>,
) -> discord_presence::PresenceServiceInfo {
    service.info()
}

#[tauri::command]
fn discord_presence_set_settings(
    app: AppHandle,
    service: State<'_, discord_presence::DiscordPresenceService>,
    settings: discord_presence::PresenceSettings,
) -> Result<(), String> {
    service.set_settings(&app, settings)
}

fn install_connection(
    connections: &Arc<Mutex<HashMap<u64, LiveConnection>>>,
    connection: LiveConnection,
) -> Result<(), String> {
    let mut connections = connections
        .lock()
        .map_err(|_| "接続状態を更新できません".to_string())?;
    connections.insert(connection.id, connection);
    Ok(())
}

fn emit_connected(app: &AppHandle, connection_id: u64) -> Result<(), String> {
    app.emit(
        "app-server-status",
        ConnectionStatus {
            connection_id,
            state: "connected",
            detail: None,
        },
    )
    .map_err(|error| format!("接続イベントを送信できません: {error}"))
}

fn spawn_direct_connection(
    app: AppHandle,
    connections: Arc<Mutex<HashMap<u64, LiveConnection>>>,
    connection_id: u64,
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut receiver: mpsc::UnboundedReceiver<Message>,
) {
    tauri::async_runtime::spawn(async move {
        let (mut writer, mut reader) = socket.split();
        let mut terminal_state = "disconnected";
        let mut terminal_detail = Some("App Serverが接続を閉じました".to_string());
        loop {
            tokio::select! {
                outbound = receiver.recv() => {
                    match outbound {
                        Some(message) => {
                            let is_close = matches!(message, Message::Close(_));
                            if let Err(error) = writer.send(message).await {
                                terminal_state = "error";
                                terminal_detail = Some(format!("送信に失敗しました: {error}"));
                                break;
                            }
                            if is_close {
                                terminal_detail = Some("切断しました".to_string());
                                break;
                            }
                        }
                        None => break,
                    }
                }
                inbound = reader.next() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<Value>(&text) {
                                Ok(message) => emit_message(&app, connection_id, message),
                                Err(error) => {
                                    terminal_state = "error";
                                    terminal_detail = Some(format!("不正なJSONを受信しました: {error}"));
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            if writer.send(Message::Pong(payload)).await.is_err() { break; }
                        }
                        Some(Ok(Message::Close(frame))) => {
                            terminal_detail = frame.map(|value| format!("接続が閉じられました: {}", value.reason));
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            terminal_state = "error";
                            terminal_detail = Some(format!("受信に失敗しました: {error}"));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        finish_connection(
            &app,
            &connections,
            connection_id,
            terminal_state,
            terminal_detail,
        );
    });
}

fn spawn_relay_connection(
    app: AppHandle,
    connections: Arc<Mutex<HashMap<u64, LiveConnection>>>,
    connection_id: u64,
    relay: pairing::PairRelay,
    stream_id: String,
    next_seq_id: Arc<AtomicU64>,
    mut receiver: mpsc::UnboundedReceiver<Message>,
) {
    tauri::async_runtime::spawn(async move {
        let client_id = relay.client_id;
        let env_id = relay.env_id;
        let (mut writer, mut reader) = relay.socket.split();
        let mut ping = tokio::time::interval(std::time::Duration::from_secs(30));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        let mut reassembler = RelayReassembler::default();
        let mut terminal_state = "disconnected";
        let mut terminal_detail = Some("公式リレーとの接続が閉じました".to_string());
        loop {
            tokio::select! {
                outbound = receiver.recv() => {
                    match outbound {
                        Some(message) => {
                            let is_close = matches!(message, Message::Close(_));
                            if let Err(error) = writer.send(message).await {
                                terminal_state = "error";
                                terminal_detail = Some(format!("公式リレーへの送信に失敗しました: {error}"));
                                break;
                            }
                            if is_close {
                                terminal_detail = Some("切断しました".to_string());
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping.tick() => {
                    let seq_id = next_seq_id.fetch_add(1, Ordering::Relaxed);
                    let envelope = json!({
                        "type": "ping",
                        "client_id": client_id,
                        "stream_id": stream_id,
                        "env_id": env_id,
                        "state": "foreground",
                        "skip_history": true,
                        "seq_id": seq_id,
                    });
                    if writer.send(Message::Text(envelope.to_string().into())).await.is_err() { break; }
                }
                inbound = reader.next() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<Value>(&text) {
                                Ok(envelope) => {
                                    if let Some(message) = unwrap_relay_message(
                                        envelope,
                                        &mut reassembler,
                                        &client_id,
                                        &env_id,
                                        &stream_id,
                                    ) {
                                        emit_message(&app, connection_id, message);
                                    }
                                }
                                Err(error) => {
                                    terminal_state = "error";
                                    terminal_detail = Some(format!("公式リレーから不正なJSONを受信しました: {error}"));
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            if writer.send(Message::Pong(payload)).await.is_err() { break; }
                        }
                        Some(Ok(Message::Close(frame))) => {
                            terminal_detail = frame.map(|value| format!("公式リレーが接続を閉じました: {}", value.reason));
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            terminal_state = "error";
                            terminal_detail = Some(format!("公式リレーからの受信に失敗しました: {error}"));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        finish_connection(
            &app,
            &connections,
            connection_id,
            terminal_state,
            terminal_detail,
        );
    });
}

fn emit_message(app: &AppHandle, connection_id: u64, message: Value) {
    let _ = app.emit(
        "app-server-message",
        IncomingEnvelope {
            connection_id,
            message,
        },
    );
}

fn finish_connection(
    app: &AppHandle,
    connections: &Arc<Mutex<HashMap<u64, LiveConnection>>>,
    connection_id: u64,
    state: &'static str,
    detail: Option<String>,
) {
    if let Ok(mut connections) = connections.lock() {
        connections.remove(&connection_id);
    }
    let _ = app.emit(
        "app-server-status",
        ConnectionStatus {
            connection_id,
            state,
            detail,
        },
    );
}

#[tauri::command]
fn send_app_server_message(
    state: State<'_, AppState>,
    connection_id: u64,
    message: Value,
) -> Result<(), String> {
    if !message.is_object() {
        return Err("App ServerメッセージはJSONオブジェクトである必要があります".to_string());
    }
    let (sender, transport) = state
        .connections
        .lock()
        .map_err(|_| "接続状態を読み取れません".to_string())?
        .get(&connection_id)
        .map(|connection| (connection.sender.clone(), connection.transport.clone()))
        .ok_or_else(|| format!("App Server接続 {connection_id} は利用できません"))?;

    match transport {
        Transport::Direct => {
            let encoded = serde_json::to_string(&message)
                .map_err(|error| format!("メッセージをエンコードできません: {error}"))?;
            sender
                .send(Message::Text(encoded.into()))
                .map_err(|_| "App Serverへの送信チャネルが閉じています".to_string())
        }
        Transport::Relay {
            client_id,
            env_id,
            stream_id,
            next_seq_id,
        } => {
            let seq_id = next_seq_id.fetch_add(1, Ordering::Relaxed);
            for envelope in
                relay_client_envelopes(&client_id, &env_id, &stream_id, seq_id, message)?
            {
                sender
                    .send(Message::Text(envelope.to_string().into()))
                    .map_err(|_| "公式リレーへの送信チャネルが閉じています".to_string())?;
            }
            Ok(())
        }
    }
}

#[tauri::command]
fn disconnect_app_server(state: State<'_, AppState>, connection_id: u64) -> Result<(), String> {
    close_connection(&state.connections, connection_id);
    Ok(())
}

fn close_connection(connections: &Arc<Mutex<HashMap<u64, LiveConnection>>>, connection_id: u64) {
    if let Ok(mut guard) = connections.lock() {
        if let Some(connection) = guard.remove(&connection_id) {
            let _ = connection.sender.send(Message::Close(None));
        }
    }
}

fn validate_endpoint(value: &str) -> Result<Url, String> {
    let endpoint = Url::parse(value.trim())
        .map_err(|_| "接続先には ws:// または wss:// の完全なURLを入力してください".to_string())?;
    match endpoint.scheme() {
        "wss" => {}
        "ws" if is_loopback_host(endpoint.host_str()) => {}
        "ws" => {
            return Err(
                "暗号化されていないws://はlocalhostまたはSSHポートフォワードにのみ使用できます"
                    .to_string(),
            )
        }
        _ => return Err("接続先のスキームはws://またはwss://にしてください".to_string()),
    }
    if endpoint.host_str().is_none() {
        return Err("接続先にホスト名がありません".to_string());
    }
    Ok(endpoint)
}

fn is_loopback_host(host: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
}

fn friendly_connect_error(error: String) -> String {
    if error.contains("401") || error.contains("403") {
        "認証に失敗しました。Bearer TokenとApp Serverの認証設定を確認してください".to_string()
    } else {
        format!("App Serverに接続できません: {error}")
    }
}

fn relay_client_envelopes(
    client_id: &str,
    env_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: Value,
) -> Result<Vec<Value>, String> {
    let envelope = json!({
        "type": "client_message",
        "client_id": client_id,
        "stream_id": stream_id,
        "env_id": env_id,
        "skip_history": false,
        "message": message,
        "seq_id": seq_id,
    });
    if envelope.to_string().len() <= RELAY_UNSEGMENTED_LIMIT {
        return Ok(vec![envelope]);
    }
    let bytes = serde_json::to_vec(&envelope["message"])
        .map_err(|error| format!("リレーメッセージを分割できません: {error}"))?;
    let chunks: Vec<_> = bytes.chunks(RELAY_CHUNK_BYTES).collect();
    let segment_count = chunks.len();
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(segment_id, chunk)| {
            json!({
                "type": "client_message_chunk",
                "client_id": client_id,
                "stream_id": stream_id,
                "env_id": env_id,
                "skip_history": false,
                "seq_id": seq_id,
                "segment_id": segment_id,
                "segment_count": segment_count,
                "message_size_bytes": bytes.len(),
                "message_chunk_base64": general_purpose::STANDARD.encode(chunk),
            })
        })
        .collect())
}

#[derive(Default)]
struct RelayReassembler {
    messages: HashMap<String, RelayAssembly>,
}

struct RelayAssembly {
    segment_count: usize,
    message_size_bytes: usize,
    chunks: Vec<Option<Vec<u8>>>,
    created_at: Instant,
}

fn unwrap_relay_message(
    envelope: Value,
    reassembler: &mut RelayReassembler,
    expected_client_id: &str,
    expected_env_id: &str,
    expected_stream_id: &str,
) -> Option<Value> {
    if !relay_envelope_matches(
        &envelope,
        expected_client_id,
        expected_env_id,
        expected_stream_id,
    ) {
        return None;
    }
    match envelope.get("type").and_then(Value::as_str) {
        Some("server_message") => envelope.get("message").cloned(),
        Some("server_message_chunk") => {
            let env_id = envelope.get("env_id")?.as_str()?;
            let stream_id = envelope.get("stream_id")?.as_str()?;
            let seq_id = envelope.get("seq_id")?.as_u64()?;
            let key = format!("{env_id}:{stream_id}:{seq_id}");
            let Some(segment_id) = envelope
                .get("segment_id")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
            else {
                reassembler.messages.remove(&key);
                return None;
            };
            let Some(segment_count) = envelope
                .get("segment_count")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
            else {
                reassembler.messages.remove(&key);
                return None;
            };
            let Some(message_size_bytes) = envelope
                .get("message_size_bytes")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
            else {
                reassembler.messages.remove(&key);
                return None;
            };
            if segment_count == 0
                || segment_count > RELAY_MAX_SEGMENTS
                || segment_id >= segment_count
                || message_size_bytes == 0
                || message_size_bytes > RELAY_MAX_MESSAGE_BYTES
            {
                reassembler.messages.remove(&key);
                return None;
            }
            let Some(encoded_chunk) = envelope.get("message_chunk_base64").and_then(Value::as_str)
            else {
                reassembler.messages.remove(&key);
                return None;
            };
            let max_encoded_chunk_bytes = RELAY_MAX_CHUNK_BYTES.div_ceil(3) * 4;
            if encoded_chunk.len() > max_encoded_chunk_bytes {
                reassembler.messages.remove(&key);
                return None;
            }
            let Some(chunk) = general_purpose::STANDARD.decode(encoded_chunk).ok() else {
                reassembler.messages.remove(&key);
                return None;
            };
            if chunk.is_empty()
                || chunk.len() > RELAY_MAX_CHUNK_BYTES
                || chunk.len() > message_size_bytes
            {
                reassembler.messages.remove(&key);
                return None;
            }
            let buffered_bytes = reassembler
                .messages
                .values()
                .flat_map(|assembly| assembly.chunks.iter())
                .filter_map(|chunk| chunk.as_ref())
                .try_fold(0_usize, |total, chunk| total.checked_add(chunk.len()))?;
            let replaced_bytes = reassembler
                .messages
                .get(&key)
                .and_then(|assembly| assembly.chunks.get(segment_id))
                .and_then(|chunk| chunk.as_ref())
                .map_or(0, Vec::len);
            let projected_bytes = buffered_bytes
                .saturating_sub(replaced_bytes)
                .checked_add(chunk.len())?;
            if projected_bytes > RELAY_MAX_BUFFERED_BYTES {
                reassembler.messages.remove(&key);
                return None;
            }
            if !reassembler.messages.contains_key(&key)
                && reassembler.messages.len() >= RELAY_MAX_ASSEMBLIES
            {
                reassembler
                    .messages
                    .retain(|_, assembly| assembly.created_at.elapsed() < RELAY_ASSEMBLY_TTL);
                if reassembler.messages.len() >= RELAY_MAX_ASSEMBLIES {
                    if let Some(oldest) = reassembler
                        .messages
                        .iter()
                        .min_by_key(|(_, assembly)| assembly.created_at)
                        .map(|(key, _)| key.clone())
                    {
                        reassembler.messages.remove(&oldest);
                    }
                }
            }
            let assembly =
                reassembler
                    .messages
                    .entry(key.clone())
                    .or_insert_with(|| RelayAssembly {
                        segment_count,
                        message_size_bytes,
                        chunks: vec![None; segment_count],
                        created_at: Instant::now(),
                    });
            if assembly.segment_count != segment_count
                || assembly.message_size_bytes != message_size_bytes
            {
                reassembler.messages.remove(&key);
                return None;
            }
            assembly.chunks[segment_id] = Some(chunk);
            let received_bytes = assembly
                .chunks
                .iter()
                .filter_map(|chunk| chunk.as_ref())
                .try_fold(0_usize, |total, chunk| total.checked_add(chunk.len()))?;
            if received_bytes > assembly.message_size_bytes {
                reassembler.messages.remove(&key);
                return None;
            }
            if assembly.chunks.iter().any(Option::is_none) {
                return None;
            }
            let assembly = reassembler.messages.remove(&key)?;
            let bytes: Vec<u8> = assembly.chunks.into_iter().flatten().flatten().collect();
            if bytes.len() != assembly.message_size_bytes {
                return None;
            }
            serde_json::from_slice(&bytes).ok()
        }
        _ => None,
    }
}

fn relay_envelope_matches(
    envelope: &Value,
    expected_client_id: &str,
    expected_env_id: &str,
    expected_stream_id: &str,
) -> bool {
    envelope.get("client_id").and_then(Value::as_str) == Some(expected_client_id)
        && envelope.get("env_id").and_then(Value::as_str) == Some(expected_env_id)
        && envelope.get("stream_id").and_then(Value::as_str) == Some(expected_stream_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android_security::init());
    let app = builder
        .manage(AppState::default())
        .setup(|app| {
            app.manage(discord_presence::DiscordPresenceService::start(
                app.handle().clone(),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_app_server,
            prepare_pair_connection,
            connect_paired_app_server,
            cancel_connection_attempt,
            record_connection_timing,
            send_app_server_message,
            disconnect_app_server,
            runtime_capabilities,
            discord_presence_update,
            discord_presence_get_settings,
            discord_presence_set_settings
        ])
        .build(tauri::generate_context!())
        .expect("failed to run hutoncodex");
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app.state::<discord_presence::DiscordPresenceService>()
                .shutdown();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_websocket_accepts_large_thread_frames_with_a_bounded_limit() {
        const OBSERVED_LARGE_THREAD_FRAME_BYTES: usize = 25_111_289;
        let config = direct_websocket_config();
        let max_frame_size = config
            .max_frame_size
            .expect("direct WebSocket frames must remain bounded");

        assert_eq!(config.max_message_size, Some(DIRECT_MAX_INCOMING_BYTES));
        assert_eq!(config.max_frame_size, Some(DIRECT_MAX_INCOMING_BYTES));
        assert!(max_frame_size > OBSERVED_LARGE_THREAD_FRAME_BYTES);
        assert!(max_frame_size <= RELAY_MAX_MESSAGE_BYTES);
    }

    #[test]
    fn disconnecting_one_connection_keeps_the_others_live() {
        let connections = Arc::new(Mutex::new(HashMap::new()));
        let (first_sender, _first_receiver) = mpsc::unbounded_channel();
        let (second_sender, _second_receiver) = mpsc::unbounded_channel();
        install_connection(
            &connections,
            LiveConnection {
                id: 11,
                sender: first_sender,
                transport: Transport::Direct,
            },
        )
        .unwrap();
        install_connection(
            &connections,
            LiveConnection {
                id: 12,
                sender: second_sender,
                transport: Transport::Direct,
            },
        )
        .unwrap();

        close_connection(&connections, 11);

        let remaining = connections.lock().unwrap();
        assert!(!remaining.contains_key(&11));
        assert!(remaining.contains_key(&12));
    }

    #[test]
    fn cancelling_one_connection_attempt_does_not_cancel_another() {
        let attempts = Arc::new(Mutex::new(HashMap::new()));
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let first = begin_connection_attempt(&attempts, &cancelled, "attempt-a").unwrap();
        let second = begin_connection_attempt(&attempts, &cancelled, "attempt-b").unwrap();
        attempts
            .lock()
            .unwrap()
            .get("attempt-a")
            .unwrap()
            .send(true)
            .unwrap();
        assert!(*first.borrow());
        assert!(!*second.borrow());
    }

    #[test]
    fn replacing_an_attempt_id_cancels_the_old_generation() {
        let attempts = Arc::new(Mutex::new(HashMap::new()));
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let old = begin_connection_attempt(&attempts, &cancelled, "same-attempt").unwrap();
        let current = begin_connection_attempt(&attempts, &cancelled, "same-attempt").unwrap();
        assert!(*old.borrow());
        assert!(!*current.borrow());
    }

    #[test]
    fn cancellation_before_attempt_registration_is_not_lost() {
        let attempts = Arc::new(Mutex::new(HashMap::new()));
        let cancelled = Arc::new(Mutex::new(HashSet::from(["early-cancel".to_string()])));

        let receiver = begin_connection_attempt(&attempts, &cancelled, "early-cancel").unwrap();

        assert!(*receiver.borrow());
        assert!(!cancelled.lock().unwrap().contains("early-cancel"));
    }

    #[test]
    fn app_state_reuses_one_http_client_instance() {
        let state = AppState::default();
        let first_reference = &state.http_client as *const Client;
        let second_reference = &state.http_client as *const Client;
        assert_eq!(first_reference, second_reference);
    }

    #[test]
    fn allows_secure_remote_and_loopback_websockets() {
        assert!(validate_endpoint("wss://codex.example.com:4500").is_ok());
        assert!(validate_endpoint("ws://127.0.0.1:4500").is_ok());
        assert!(validate_endpoint("ws://[::1]:4500").is_ok());
        assert!(validate_endpoint("ws://localhost:4500").is_ok());
    }

    #[test]
    fn rejects_plaintext_remote_endpoints() {
        let error = validate_endpoint("ws://192.168.1.25:4500").unwrap_err();
        assert!(error.contains("localhost"));
        assert!(validate_endpoint("https://codex.example.com").is_err());
    }

    #[test]
    fn relay_messages_are_wrapped_and_chunked() {
        let small = relay_client_envelopes("client", "env", "stream", 1, json!({"id": 1})).unwrap();
        assert_eq!(small[0]["type"], "client_message");
        let large = relay_client_envelopes(
            "client",
            "env",
            "stream",
            2,
            json!({"method": "turn/start", "params": {"input": "x".repeat(180_000)}}),
        )
        .unwrap();
        assert!(large.len() > 1);
        assert!(large
            .iter()
            .all(|value| value["type"] == "client_message_chunk"));
    }

    #[test]
    fn relay_rejects_another_environment_or_stream() {
        let envelope = json!({
            "type": "server_message",
            "client_id": "client",
            "env_id": "env-b",
            "stream_id": "stream-a",
            "message": {"id": 1, "result": {}}
        });
        let mut reassembler = RelayReassembler::default();
        assert!(
            unwrap_relay_message(envelope, &mut reassembler, "client", "env-a", "stream-a",)
                .is_none()
        );
    }

    #[test]
    fn relay_accepts_a_matching_chunked_message() {
        let message = json!({"id": 1, "result": {"ok": true}});
        let bytes = serde_json::to_vec(&message).unwrap();
        let envelope = json!({
            "type": "server_message_chunk",
            "client_id": "client",
            "env_id": "env",
            "stream_id": "stream",
            "seq_id": 1,
            "segment_id": 0,
            "segment_count": 1,
            "message_size_bytes": bytes.len(),
            "message_chunk_base64": general_purpose::STANDARD.encode(bytes)
        });
        let mut reassembler = RelayReassembler::default();
        assert_eq!(
            unwrap_relay_message(envelope, &mut reassembler, "client", "env", "stream",),
            Some(message)
        );
    }

    #[test]
    fn relay_rejects_huge_segment_count_without_allocating() {
        let envelope = json!({
            "type": "server_message_chunk",
            "client_id": "client",
            "env_id": "env",
            "stream_id": "stream",
            "seq_id": 1,
            "segment_id": 0,
            "segment_count": 1_000_000_000_u64,
            "message_size_bytes": 1,
            "message_chunk_base64": "eA=="
        });
        let mut reassembler = RelayReassembler::default();
        assert!(
            unwrap_relay_message(envelope, &mut reassembler, "client", "env", "stream",).is_none()
        );
        assert!(reassembler.messages.is_empty());
    }
}
