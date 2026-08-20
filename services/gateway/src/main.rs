use anyhow::{bail, Context};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ws::Message, ws::WebSocket, Path, Query, State, WebSocketUpgrade},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use rand::{rngs::OsRng, RngCore};
use remote_protocol::{
    AgentToGateway, BrowserSessionId, GatewayToAgent, HostId, UserId, GATEWAY_PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use session_core::SessionRegistry;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::{
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing::{info, warn};
use uuid::Uuid;

const SESSION_COOKIE: &str = "codex_remote_session";
const MAX_API_BODY_BYTES: usize = 1024 * 1024;
const MAX_AGENT_FRAME_BYTES: usize = 32 * 1024 * 1024;
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const WS_TICKET_TTL: Duration = Duration::from_secs(60);
const WS_PROTOCOL: &str = "codex-remote-v1";

#[derive(Parser, Debug)]
#[command(name = "codex-remote-gateway")]
struct Args {
    #[arg(long, env = "CODEX_REMOTE_BIND", default_value = "127.0.0.1:8787")]
    bind: SocketAddr,
    #[arg(long, env = "CODEX_REMOTE_SECURE_COOKIES", default_value_t = false)]
    secure_cookies: bool,
    #[arg(long, env = "CODEX_REMOTE_WEB_DIST", default_value = "dist")]
    web_dist: String,
    #[arg(long, env = "CODEX_REMOTE_PUBLIC_ORIGIN")]
    public_origin: Option<String>,
}

#[derive(Clone)]
struct GatewayState {
    admin_user_id: UserId,
    password_hash: Arc<str>,
    host_token_hash: [u8; 32],
    secure_cookies: bool,
    public_origin: Arc<str>,
    sessions: Arc<RwLock<HashMap<String, WebSession>>>,
    ws_tickets: Arc<RwLock<HashMap<String, WsTicket>>>,
    registry: SessionRegistry,
    events: broadcast::Sender<OwnedEvent>,
}

#[derive(Clone)]
struct WsTicket {
    browser_session_id: BrowserSessionId,
    expires_at: tokio::time::Instant,
}

#[derive(Clone)]
struct WebSession {
    id: BrowserSessionId,
    user_id: UserId,
    csrf_token: String,
    expires_at: tokio::time::Instant,
}

#[derive(Clone, Debug)]
struct OwnedEvent {
    owner_user_id: UserId,
    browser_session_id: Option<BrowserSessionId>,
    text: String,
}

#[derive(Debug)]
enum ApiError {
    Unauthorized,
    Forbidden,
    BadRequest(&'static str),
    Conflict(&'static str),
    ServiceUnavailable(&'static str),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "authentication required"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "request is not authorized"),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::ServiceUnavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message),
        };
        (status, Json(json!({ "error": { "message": message } }))).into_response()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    csrf_token: String,
    user_id: UserId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    user_id: UserId,
    csrf_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WsTicketResponse {
    ticket: String,
    protocol: &'static str,
    expires_in_seconds: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentQuery {
    generation: u64,
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserRpcRequest {
    generation: u64,
    message: Value,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codex_remote_gateway=info,tower_http=info".into()),
        )
        .init();
    let args = Args::parse();
    if !args.bind.ip().is_loopback() && !args.secure_cookies {
        bail!("non-loopback binding requires --secure-cookies behind an HTTPS reverse proxy");
    }
    let public_origin = args
        .public_origin
        .unwrap_or_else(|| format!("http://{}", args.bind));
    validate_public_origin(&public_origin, args.bind, args.secure_cookies)?;
    let password = std::env::var("CODEX_REMOTE_ADMIN_PASSWORD")
        .context("CODEX_REMOTE_ADMIN_PASSWORD is required")?;
    if password.len() < 12 {
        bail!("CODEX_REMOTE_ADMIN_PASSWORD must contain at least 12 characters");
    }
    let host_token =
        std::env::var("CODEX_REMOTE_HOST_TOKEN").context("CODEX_REMOTE_HOST_TOKEN is required")?;
    if host_token.len() < 32 {
        bail!("CODEX_REMOTE_HOST_TOKEN must contain at least 32 characters");
    }
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?
        .to_string();
    let host_token_hash: [u8; 32] = Sha256::digest(host_token.as_bytes()).into();
    let (events, _) = broadcast::channel(1024);
    let state = GatewayState {
        admin_user_id: UserId(Uuid::new_v4()),
        password_hash: password_hash.into(),
        host_token_hash,
        secure_cookies: args.secure_cookies,
        public_origin: public_origin.into(),
        sessions: Arc::default(),
        ws_tickets: Arc::default(),
        registry: SessionRegistry::default(),
        events,
    };
    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/session/login", post(login))
        .route("/api/v1/session/logout", post(logout))
        .route("/api/v1/session/me", get(session_me))
        .route("/api/v1/hosts", get(list_hosts))
        .route("/api/v1/hosts/{host_id}/rpc", post(browser_rpc))
        .route("/api/v1/events", get(browser_events))
        .route("/api/v1/events/ticket", post(create_ws_ticket))
        .route("/api/v1/agent/connect/{host_id}", get(agent_connect))
        .fallback_service(
            ServeDir::new(&args.web_dist)
                .append_index_html_on_directories(true)
                .not_found_service(ServeFile::new(format!("{}/index.html", args.web_dist))),
        )
        .layer(RequestBodyLimitLayer::new(MAX_API_BODY_BYTES))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!(bind = %args.bind, "gateway listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "protocolVersion": GATEWAY_PROTOCOL_VERSION }))
}

async fn login(
    State(state): State<GatewayState>,
    Json(request): Json<LoginRequest>,
) -> Result<(HeaderMap, Json<LoginResponse>), ApiError> {
    let parsed = PasswordHash::new(&state.password_hash).map_err(|_| ApiError::Unauthorized)?;
    Argon2::default()
        .verify_password(request.password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Unauthorized)?;
    let session_token = random_token(32);
    let csrf_token = random_token(32);
    let session = WebSession {
        id: BrowserSessionId(Uuid::new_v4()),
        user_id: state.admin_user_id.clone(),
        csrf_token: csrf_token.clone(),
        expires_at: tokio::time::Instant::now() + SESSION_TTL,
    };
    state
        .registry
        .register_browser(session.id.clone(), session.user_id.clone())
        .await;
    state
        .sessions
        .write()
        .await
        .insert(session_token.clone(), session);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&session_token, state.secure_cookies))
            .map_err(|_| ApiError::BadRequest("could not create session cookie"))?,
    );
    Ok((
        headers,
        Json(LoginResponse {
            csrf_token,
            user_id: state.admin_user_id,
        }),
    ))
}

async fn session_me(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Result<Json<SessionResponse>, ApiError> {
    let (_, session) = authenticated_session(&state, &headers).await?;
    Ok(Json(SessionResponse {
        user_id: session.user_id,
        csrf_token: session.csrf_token,
    }))
}

async fn logout(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Result<(HeaderMap, StatusCode), ApiError> {
    let (token, session) = authenticated_session_with_csrf(&state, &headers).await?;
    state.sessions.write().await.remove(&token);
    state.registry.unregister_browser(&session.id).await;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&expired_session_cookie(state.secure_cookies))
            .map_err(|_| ApiError::BadRequest("could not expire session cookie"))?,
    );
    Ok((response_headers, StatusCode::NO_CONTENT))
}

async fn list_hosts(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let (_, session) = authenticated_session(&state, &headers).await?;
    let hosts = state.registry.list_hosts_for_user(&session.user_id).await;
    Ok(Json(json!({ "data": hosts.into_iter().map(|host| json!({
        "id": host.host_id,
        "displayName": host.display_name,
        "generation": host.generation,
        "lunaMax": host.luna_max,
        "state": "appServerReady"
    })).collect::<Vec<_>>() })))
}

async fn create_ws_ticket(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Result<Json<WsTicketResponse>, ApiError> {
    let (_, session) = authenticated_session_with_csrf(&state, &headers).await?;
    let ticket = random_token(32);
    state.ws_tickets.write().await.insert(
        ticket.clone(),
        WsTicket {
            browser_session_id: session.id,
            expires_at: tokio::time::Instant::now() + WS_TICKET_TTL,
        },
    );
    Ok(Json(WsTicketResponse {
        ticket,
        protocol: WS_PROTOCOL,
        expires_in_seconds: WS_TICKET_TTL.as_secs(),
    }))
}

async fn browser_rpc(
    State(state): State<GatewayState>,
    Path(host_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<BrowserRpcRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, session) = authenticated_session_with_csrf(&state, &headers).await?;
    validate_browser_rpc(&request.message)?;
    let host_id = HostId(host_id);
    let sender = state
        .registry
        .route_for_browser(&session.id, &host_id, request.generation)
        .await
        .map_err(|error| match error {
            session_core::RouteError::Forbidden => ApiError::Forbidden,
            session_core::RouteError::StaleGeneration => {
                ApiError::Conflict("host connection generation is stale")
            }
            _ => ApiError::ServiceUnavailable("host agent is not connected"),
        })?;
    let frame = GatewayToAgent::AppServerMessage {
        browser_session_id: session.id,
        host_id,
        generation: request.generation,
        message: request.message,
    };
    sender
        .send(serde_json::to_string(&frame).map_err(|_| ApiError::BadRequest("invalid RPC"))?)
        .await
        .map_err(|_| ApiError::ServiceUnavailable("host agent connection closed"))?;
    Ok(StatusCode::ACCEPTED)
}

async fn browser_events(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (session_token, session) = authenticated_session(&state, &headers).await?;
    verify_origin(&state, &headers)?;
    let ticket = websocket_ticket(&headers).ok_or(ApiError::Forbidden)?;
    let issued = state
        .ws_tickets
        .write()
        .await
        .remove(ticket)
        .ok_or(ApiError::Forbidden)?;
    if issued.expires_at <= tokio::time::Instant::now() || issued.browser_session_id != session.id {
        return Err(ApiError::Forbidden);
    }
    Ok(upgrade
        .protocols([WS_PROTOCOL])
        .max_message_size(MAX_API_BODY_BYTES)
        .on_upgrade(move |socket| browser_event_socket(socket, state, session_token, session))
        .into_response())
}

async fn browser_event_socket(
    socket: WebSocket,
    state: GatewayState,
    session_token: String,
    session: WebSession,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();
    let mut session_check = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = session_check.tick() => {
                let active = state.sessions.read().await.get(&session_token).is_some_and(|current| {
                    current.id == session.id && current.expires_at > tokio::time::Instant::now()
                });
                if !active { break; }
            }
            event = events.recv() => {
                match event {
                    Ok(event) if event.owner_user_id == session.user_id
                        && event.browser_session_id.as_ref().is_none_or(|id| id == &session.id) => {
                        if sender.send(Message::Text(event.text.into())).await.is_err() { break; }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = sender.send(Message::Text(json!({"type":"resyncRequired"}).to_string().into())).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Ping(bytes))) => {
                        if sender.send(Message::Pong(bytes)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn agent_connect(
    State(state): State<GatewayState>,
    Path(host_id): Path<Uuid>,
    Query(query): Query<AgentQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    verify_host_token(&state, &headers)?;
    let display_name = query
        .display_name
        .unwrap_or_else(|| "Codex Host".to_string());
    if display_name.is_empty() || display_name.len() > 120 {
        return Err(ApiError::BadRequest("invalid host display name"));
    }
    let host_id = HostId(host_id);
    Ok(upgrade
        .max_message_size(MAX_AGENT_FRAME_BYTES)
        .on_upgrade(move |socket| {
            agent_socket(socket, state, host_id, query.generation, display_name)
        })
        .into_response())
}

async fn agent_socket(
    socket: WebSocket,
    state: GatewayState,
    host_id: HostId,
    generation: u64,
    display_name: String,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let hello = tokio::time::timeout(Duration::from_secs(60), ws_receiver.next()).await;
    let Ok(Some(Ok(Message::Text(text)))) = hello else {
        warn!(host_id = %host_id.0, "host did not provide a timely hello");
        return;
    };
    let Ok(AgentToGateway::Hello {
        protocol_version,
        host_id: claimed,
        generation: claimed_generation,
        display_name: claimed_display_name,
    }) = serde_json::from_str::<AgentToGateway>(&text)
    else {
        warn!(host_id = %host_id.0, "host provided an invalid hello");
        return;
    };
    if protocol_version != GATEWAY_PROTOCOL_VERSION
        || claimed != host_id
        || claimed_generation != generation
        || claimed_display_name != display_name
    {
        warn!(host_id = %host_id.0, "host hello does not match the authenticated route");
        return;
    }
    let (route_sender, mut route_receiver) = mpsc::channel::<String>(256);
    if let Err(error) = state
        .registry
        .register_host(
            host_id.clone(),
            state.admin_user_id.clone(),
            generation,
            display_name.clone(),
            route_sender,
        )
        .await
    {
        warn!(?error, "host registration rejected");
        return;
    }
    info!(host_id = %host_id.0, generation, display_name, "host connected");
    loop {
        tokio::select! {
            outgoing = route_receiver.recv() => {
                let Some(outgoing) = outgoing else { break; };
                if ws_sender.send(Message::Text(outgoing.into())).await.is_err() { break; }
            }
            incoming = ws_receiver.next() => {
                let Some(Ok(Message::Text(text))) = incoming else { break; };
                let Ok(frame) = serde_json::from_str::<AgentToGateway>(&text) else { continue; };
                if !state.registry.accepts_host_event(&host_id, generation).await { break; }
                let event = match frame {
                    AgentToGateway::AppServerMessage { envelope }
                        if envelope.host_id == host_id && envelope.connection_generation == generation => {
                        let browser_session_id = envelope.browser_session_id.clone();
                        Some(OwnedEvent {
                            owner_user_id: state.admin_user_id.clone(),
                            browser_session_id,
                            text: serde_json::to_string(&AgentToGateway::AppServerMessage { envelope }).unwrap_or_default(),
                        })
                    }
                    AgentToGateway::Status { host_id: claimed, generation: claimed_generation, state: connection_state, detail }
                        if claimed == host_id && claimed_generation == generation => {
                        Some(OwnedEvent {
                            owner_user_id: state.admin_user_id.clone(),
                            browser_session_id: None,
                            text: serde_json::to_string(&AgentToGateway::Status {
                                host_id: claimed,
                                generation: claimed_generation,
                                state: connection_state,
                                detail,
                            }).unwrap_or_default(),
                        })
                    }
                    AgentToGateway::Capabilities { host_id: claimed, generation: claimed_generation, luna_max }
                        if claimed == host_id && claimed_generation == generation => {
                        if state.registry.update_host_capabilities(
                            &claimed,
                            claimed_generation,
                            luna_max.clone(),
                        ).await.is_err() {
                            break;
                        }
                        Some(OwnedEvent {
                            owner_user_id: state.admin_user_id.clone(),
                            browser_session_id: None,
                            text: serde_json::to_string(&AgentToGateway::Capabilities {
                                host_id: claimed,
                                generation: claimed_generation,
                                luna_max,
                            }).unwrap_or_default(),
                        })
                    }
                    _ => break,
                };
                if let Some(event) = event { let _ = state.events.send(event); }
            }
        }
    }
    state.registry.unregister_host(&host_id, generation).await;
}

async fn authenticated_session(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<(String, WebSession), ApiError> {
    let token = cookie_value(headers, SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
    let session = state
        .sessions
        .read()
        .await
        .get(&token)
        .cloned()
        .ok_or(ApiError::Unauthorized)?;
    if session.expires_at <= tokio::time::Instant::now() {
        state.sessions.write().await.remove(&token);
        state.registry.unregister_browser(&session.id).await;
        return Err(ApiError::Unauthorized);
    }
    Ok((token, session))
}

async fn authenticated_session_with_csrf(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<(String, WebSession), ApiError> {
    let authenticated = authenticated_session(state, headers).await?;
    let supplied = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Forbidden)?;
    if !constant_time_equal(supplied.as_bytes(), authenticated.1.csrf_token.as_bytes()) {
        return Err(ApiError::Forbidden);
    }
    Ok(authenticated)
}

fn verify_host_token(state: &GatewayState, headers: &HeaderMap) -> Result<(), ApiError> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    let candidate: [u8; 32] = Sha256::digest(authorization.as_bytes()).into();
    if !bool::from(candidate.ct_eq(&state.host_token_hash)) {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

fn validate_browser_rpc(message: &Value) -> Result<(), ApiError> {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        if !allowed_app_server_method(method) {
            return Err(ApiError::Forbidden);
        }
        let id =
            message
                .get("id")
                .filter(|id| valid_json_rpc_id(id))
                .ok_or(ApiError::BadRequest(
                    "client requests require a string or number id",
                ))?;
        let _ = id;
        return Ok(());
    }
    let valid_id = message.get("id").is_some_and(valid_json_rpc_id);
    let has_result = message.get("result").is_some();
    let has_error = message.get("error").is_some();
    if valid_id && has_result != has_error {
        return Ok(());
    }
    Err(ApiError::BadRequest("invalid JSON-RPC message"))
}

fn valid_json_rpc_id(value: &Value) -> bool {
    value.as_str().is_some() || value.as_i64().is_some() || value.as_u64().is_some()
}

fn allowed_app_server_method(method: &str) -> bool {
    matches!(
        method,
        "thread/list"
            | "thread/search"
            | "thread/read"
            | "thread/start"
            | "thread/resume"
            | "thread/archive"
            | "thread/delete"
            | "turn/start"
            | "turn/steer"
            | "turn/interrupt"
            | "model/list"
            | "account/rateLimits/read"
            | "account/usage/read"
            | "collaborationMode/list"
    )
}

fn websocket_ticket(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)?
        .to_str()
        .ok()?
        .split(',')
        .map(str::trim)
        .find(|protocol| *protocol != WS_PROTOCOL && !protocol.is_empty())
}

fn verify_origin(state: &GatewayState, headers: &HeaderMap) -> Result<(), ApiError> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Forbidden)?;
    if !constant_time_equal(origin.as_bytes(), state.public_origin.as_bytes()) {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

fn validate_public_origin(
    origin: &str,
    bind: SocketAddr,
    secure_cookies: bool,
) -> anyhow::Result<()> {
    let parsed = url::Url::parse(origin).context("CODEX_REMOTE_PUBLIC_ORIGIN is invalid")?;
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("CODEX_REMOTE_PUBLIC_ORIGIN must contain only scheme and authority");
    }
    if secure_cookies && parsed.scheme() != "https" {
        bail!("secure cookies require an https public origin");
    }
    if !secure_cookies && (!bind.ip().is_loopback() || parsed.scheme() != "http") {
        bail!("insecure development origin is allowed only for loopback http");
    }
    Ok(())
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_string()))
}

fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={};{}",
        SESSION_TTL.as_secs(),
        if secure { " Secure;" } else { "" }
    )
}

fn expired_session_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0;{}",
        if secure { " Secure;" } else { "" }
    )
}

fn random_token(bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unapproved_rpc_methods() {
        assert!(matches!(
            validate_browser_rpc(&json!({"id": 1, "method": "fs/remove", "params": {}})),
            Err(ApiError::Forbidden)
        ));
    }

    #[test]
    fn accepts_required_turn_interrupt() {
        assert!(validate_browser_rpc(&json!({
            "id": 1,
            "method": "turn/interrupt",
            "params": {"threadId":"t", "turnId":"u"}
        }))
        .is_ok());
    }

    #[test]
    fn secure_cookie_is_opt_in_for_loopback_development() {
        assert!(!session_cookie("token", false).contains(" Secure;"));
        assert!(session_cookie("token", true).contains(" Secure;"));
    }

    #[test]
    fn browser_cannot_call_token_bearing_auth_methods() {
        assert!(matches!(
            validate_browser_rpc(&json!({"id": 1, "method": "account/login/start", "params": {}})),
            Err(ApiError::Forbidden)
        ));
        assert!(matches!(
            validate_browser_rpc(&json!({"id": 2, "method": "getAuthStatus", "params": {}})),
            Err(ApiError::Forbidden)
        ));
    }

    #[test]
    fn websocket_ticket_is_carried_outside_the_url() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("codex-remote-v1, one-time-ticket"),
        );
        assert_eq!(websocket_ticket(&headers), Some("one-time-ticket"));
    }

    #[test]
    fn public_origin_requires_https_for_secure_cookies() {
        let loopback: SocketAddr = "127.0.0.1:8787".parse().unwrap();
        assert!(validate_public_origin("https://remote.example", loopback, true).is_ok());
        assert!(validate_public_origin("http://remote.example", loopback, true).is_err());
    }
}
