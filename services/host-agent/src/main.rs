use anyhow::{bail, Context};
use app_server_client::AppServerProcess;
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use http::header::{HeaderValue, AUTHORIZATION};
use remote_protocol::{
    detect_luna_max, AgentToGateway, AppServerEnvelope, CatalogModel, GatewayToAgent,
    HostConnectionState, HostId, LunaMaxCapability, GATEWAY_PROTOCOL_VERSION,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "codex-remote-agent", about = "Codex Remote Host Agent")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Doctor,
    Connect {
        #[arg(long, env = "CODEX_REMOTE_GATEWAY")]
        gateway: Url,
        #[arg(long, env = "CODEX_REMOTE_HOST_ID")]
        host_id: Uuid,
        #[arg(long, default_value = "Codex Host")]
        display_name: String,
        #[arg(long, default_value_t = 1)]
        generation: u64,
        #[arg(long = "workspace", required = true)]
        workspaces: Vec<PathBuf>,
    },
    Workspaces {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    AppServer {
        #[command(subcommand)]
        command: AppServerCommand,
    },
}

#[derive(Subcommand, Debug)]
enum WorkspaceCommand {
    List {
        #[arg(long = "workspace", required = true)]
        workspaces: Vec<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum AppServerCommand {
    Probe,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorReport {
    codex_found: bool,
    host_token_available: bool,
    gateway_configured: bool,
    protocol_version: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codex_remote_agent=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => doctor(cli.json).await,
        Command::Connect {
            gateway,
            host_id,
            display_name,
            generation,
            workspaces,
        } => connect(gateway, host_id, display_name, generation, workspaces).await,
        Command::Workspaces {
            command: WorkspaceCommand::List { workspaces },
        } => list_workspaces(workspaces, cli.json),
        Command::AppServer {
            command: AppServerCommand::Probe,
        } => probe_app_server(cli.json).await,
    }
}

async fn doctor(json_output: bool) -> anyhow::Result<()> {
    let codex_found = tokio::process::Command::new("codex")
        .arg("--version")
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    let report = DoctorReport {
        codex_found,
        host_token_available: std::env::var("CODEX_REMOTE_HOST_TOKEN")
            .is_ok_and(|value| value.len() >= 32),
        gateway_configured: std::env::var("CODEX_REMOTE_GATEWAY").is_ok(),
        protocol_version: GATEWAY_PROTOCOL_VERSION,
    };
    if json_output {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "codex: {}",
            if report.codex_found { "ok" } else { "missing" }
        );
        println!(
            "host token: {}",
            if report.host_token_available {
                "configured"
            } else {
                "missing"
            }
        );
        println!(
            "gateway: {}",
            if report.gateway_configured {
                "configured"
            } else {
                "missing"
            }
        );
    }
    if !report.codex_found {
        bail!("codex CLI is not available");
    }
    Ok(())
}

fn list_workspaces(workspaces: Vec<PathBuf>, json_output: bool) -> anyhow::Result<()> {
    let canonical = canonical_workspaces(&workspaces)?;
    if json_output {
        println!("{}", serde_json::to_string(&json!({ "data": canonical }))?);
    } else {
        for path in canonical {
            println!("{}", path.display());
        }
    }
    Ok(())
}

async fn probe_app_server(json_output: bool) -> anyhow::Result<()> {
    let app_server = AppServerProcess::spawn().await?;
    let initialize = app_server.initialize().await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({ "ok": true, "initialize": initialize }))?
        );
    } else {
        println!("Codex App Server initialized successfully");
    }
    app_server.shutdown().await?;
    Ok(())
}

async fn connect(
    mut gateway: Url,
    host_id: Uuid,
    display_name: String,
    generation: u64,
    workspaces: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let allowed_workspaces = canonical_workspaces(&workspaces)?;
    validate_gateway_url(&gateway)?;
    let host_token =
        std::env::var("CODEX_REMOTE_HOST_TOKEN").context("CODEX_REMOTE_HOST_TOKEN is required")?;
    if host_token.len() < 32 {
        bail!("CODEX_REMOTE_HOST_TOKEN must contain at least 32 characters");
    }
    gateway.set_path(&format!("/api/v1/agent/connect/{host_id}"));
    gateway.set_query(Some(&format!(
        "generation={generation}&displayName={}",
        url::form_urlencoded::byte_serialize(display_name.as_bytes()).collect::<String>()
    )));
    let mut request = gateway.as_str().into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {host_token}"))?,
    );
    let (socket, _) = connect_async(request).await?;
    let (mut gateway_sender, mut gateway_receiver) = socket.split();
    let app_server = AppServerProcess::spawn().await?;
    let app_server_process_id = Uuid::new_v4();
    let initialize = app_server.initialize().await?;
    info!(?initialize, "app-server initialized");
    let luna_max = load_luna_max_capability(&app_server).await;
    let host_id = HostId(host_id);
    gateway_sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&AgentToGateway::Hello {
                protocol_version: GATEWAY_PROTOCOL_VERSION,
                host_id: host_id.clone(),
                generation,
                display_name,
            })?
            .into(),
        ))
        .await?;
    gateway_sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&AgentToGateway::Status {
                host_id: host_id.clone(),
                generation,
                state: HostConnectionState::AppServerReady,
                detail: None,
            })?
            .into(),
        ))
        .await?;
    gateway_sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&AgentToGateway::Capabilities {
                host_id: host_id.clone(),
                generation,
                luna_max,
            })?
            .into(),
        ))
        .await?;
    let mut app_server_events = app_server.subscribe();
    let mut sequence = 0_u64;
    let mut rewritten_ids: HashMap<String, (remote_protocol::BrowserSessionId, Value)> =
        HashMap::new();
    let mut thread_owners: HashMap<String, remote_protocol::BrowserSessionId> = HashMap::new();
    let mut server_request_owners: HashMap<String, remote_protocol::BrowserSessionId> =
        HashMap::new();
    loop {
        tokio::select! {
            frame = gateway_receiver.next() => {
                let Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) = frame else { break; };
                let frame: GatewayToAgent = serde_json::from_str(&text)?;
                match frame {
                    GatewayToAgent::AppServerMessage { browser_session_id, host_id: target, generation: target_generation, mut message }
                        if target == host_id && target_generation == generation => {
                        if let Err(error) = validate_workspace_message(&message, &allowed_workspaces) {
                            if let Some(id) = message.get("id").cloned() {
                                sequence = sequence.saturating_add(1);
                                let rejection = AgentToGateway::AppServerMessage {
                                    envelope: AppServerEnvelope {
                                        host_id: host_id.clone(),
                                        browser_session_id: Some(browser_session_id.clone()),
                                        app_server_process_id,
                                        connection_generation: generation,
                                        sequence,
                                        message: json!({ "id": id, "error": { "code": -32602, "message": error.to_string() } }),
                                    },
                                };
                                gateway_sender.send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&rejection)?.into()
                                )).await?;
                            }
                            continue;
                        }
                        if message.get("method").is_none() {
                            let Some(id) = message.get("id").map(json_id_key) else {
                                warn!("discarded malformed server response");
                                continue;
                            };
                            if server_request_owners.remove(&id).as_ref() != Some(&browser_session_id) {
                                warn!("discarded server response from a browser that does not own the request");
                                continue;
                            }
                        }
                        if let Some(thread_id) = message_thread_id(&message) {
                            if !claim_thread_owner(&mut thread_owners, thread_id, &browser_session_id) {
                                warn!(thread_id, "discarded a cross-session thread ownership attempt");
                                continue;
                            }
                        }
                        if message.get("method").is_some() {
                            if let Some(original_id) = message.get("id").cloned() {
                                let rewritten = format!("web:{}:{}", browser_session_id.0, Uuid::new_v4());
                                rewritten_ids.insert(rewritten.clone(), (browser_session_id, original_id));
                                message["id"] = Value::String(rewritten);
                            }
                        }
                        app_server.send_raw(&message).await?;
                    }
                    GatewayToAgent::Shutdown { reason } => {
                        info!(reason, "gateway requested shutdown");
                        break;
                    }
                    _ => warn!("discarded a frame for another host generation"),
                }
            }
            app_event = app_server_events.recv() => {
                let Ok(mut message) = app_event else { continue; };
                let mut browser_session_id = None;
                if message.get("method").is_none() {
                    if let Some(rewritten) = message.get("id").and_then(Value::as_str).map(str::to_string) {
                        if let Some((session_id, original_id)) = rewritten_ids.remove(&rewritten) {
                            if let Some(thread_id) = message
                                .pointer("/result/thread/id")
                                .and_then(Value::as_str)
                            {
                                let _ = claim_thread_owner(&mut thread_owners, thread_id, &session_id);
                            }
                            message["id"] = original_id;
                            browser_session_id = Some(session_id);
                        }
                    }
                } else {
                    let method = message.get("method").and_then(Value::as_str).unwrap_or_default();
                    if message.get("id").is_some()
                        && matches!(method, "account/chatgptAuthTokens/refresh" | "attestation/generate" | "currentTime/read")
                    {
                        let response = json!({
                            "id": message["id"].clone(),
                            "error": { "code": -32601, "message": "request is not implemented by the Host Agent" }
                        });
                        app_server.send_raw(&response).await?;
                        continue;
                    }
                    if let Some(thread_id) = message_thread_id(&message) {
                        browser_session_id = thread_owners.get(thread_id).cloned();
                    }
                    if message.get("id").is_some() {
                        let Some(owner) = browser_session_id.clone() else {
                            let response = json!({
                                "id": message["id"].clone(),
                                "error": { "code": -32000, "message": "no authenticated browser session owns this request" }
                            });
                            app_server.send_raw(&response).await?;
                            continue;
                        };
                        server_request_owners.insert(json_id_key(&message["id"]), owner);
                    }
                }
                sequence = sequence.saturating_add(1);
                let frame = AgentToGateway::AppServerMessage {
                    envelope: AppServerEnvelope {
                        host_id: host_id.clone(),
                        browser_session_id,
                        app_server_process_id,
                        connection_generation: generation,
                        sequence,
                        message,
                    },
                };
                gateway_sender.send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&frame)?.into()
                )).await?;
            }
        }
    }
    app_server.shutdown().await?;
    Ok(())
}

fn json_id_key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn message_thread_id(message: &Value) -> Option<&str> {
    message
        .pointer("/params/threadId")
        .or_else(|| message.pointer("/params/conversationId"))
        .and_then(Value::as_str)
}

fn claim_thread_owner(
    owners: &mut HashMap<String, remote_protocol::BrowserSessionId>,
    thread_id: &str,
    browser_session_id: &remote_protocol::BrowserSessionId,
) -> bool {
    match owners.get(thread_id) {
        Some(owner) => owner == browser_session_id,
        None => {
            owners.insert(thread_id.to_string(), browser_session_id.clone());
            true
        }
    }
}

fn validate_workspace_message(message: &Value, allowed: &[PathBuf]) -> anyhow::Result<()> {
    if message.get("method").and_then(Value::as_str) == Some("thread/start") {
        let cwd = message
            .pointer("/params/cwd")
            .and_then(Value::as_str)
            .context("thread/start requires a workspace cwd")?;
        validate_workspace_path(PathBuf::from(cwd), allowed)?;
    }
    if message.get("method").and_then(Value::as_str) == Some("turn/start") {
        if let Some(input) = message.pointer("/params/input").and_then(Value::as_array) {
            for item in input {
                if item.get("type").and_then(Value::as_str) == Some("localImage") {
                    let path = item
                        .get("path")
                        .and_then(Value::as_str)
                        .context("localImage requires a path")?;
                    validate_workspace_path(PathBuf::from(path), allowed)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_workspace_path(path: PathBuf, allowed: &[PathBuf]) -> anyhow::Result<()> {
    if !path.is_absolute() {
        bail!("workspace path must be absolute");
    }
    let canonical = path
        .canonicalize()
        .context("workspace path does not exist")?;
    if !allowed.iter().any(|root| canonical.starts_with(root)) {
        bail!("path is outside every registered workspace");
    }
    Ok(())
}

async fn load_luna_max_capability(app_server: &AppServerProcess) -> LunaMaxCapability {
    let mut cursor: Option<String> = None;
    let mut models = Vec::<CatalogModel>::new();
    let mut seen_cursors = std::collections::HashSet::new();
    for _ in 0..100 {
        let response = match app_server
            .request_value("model/list", json!({ "limit": 100, "cursor": cursor }))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return LunaMaxCapability::Unavailable {
                    reason: format!("model/list の取得に失敗しました: {error}"),
                };
            }
        };
        let Some(page) = response
            .get("data")
            .cloned()
            .and_then(|value| serde_json::from_value::<Vec<CatalogModel>>(value).ok())
        else {
            return LunaMaxCapability::Unavailable {
                reason: "model/list の応答形式を検証できません".to_string(),
            };
        };
        models.extend(page);
        let next = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(next) = next else {
            return detect_luna_max(&models);
        };
        if !seen_cursors.insert(next.clone()) {
            return LunaMaxCapability::Unavailable {
                reason: "model/list のcursorが循環しました".to_string(),
            };
        }
        cursor = Some(next);
    }
    LunaMaxCapability::Unavailable {
        reason: "model/list がページ上限を超えました".to_string(),
    }
}

fn canonical_workspaces(workspaces: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut result = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        if !workspace.is_absolute() {
            bail!("workspace paths must be absolute: {}", workspace.display());
        }
        let canonical = workspace
            .canonicalize()
            .with_context(|| format!("workspace does not exist: {}", workspace.display()))?;
        if !canonical.is_dir() {
            bail!("workspace is not a directory: {}", canonical.display());
        }
        result.push(canonical);
    }
    result.sort();
    result.dedup();
    Ok(result)
}

fn validate_gateway_url(url: &Url) -> anyhow::Result<()> {
    match url.scheme() {
        "wss" => Ok(()),
        "ws" if url.host_str().is_some_and(is_loopback_host) => Ok(()),
        "ws" => bail!("plaintext ws:// is allowed only for a loopback gateway"),
        _ => bail!("gateway URL must use wss:// or loopback ws://"),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_plaintext_gateway_is_rejected() {
        assert!(validate_gateway_url(&Url::parse("ws://192.0.2.10:8787").unwrap()).is_err());
        assert!(validate_gateway_url(&Url::parse("ws://127.0.0.1:8787").unwrap()).is_ok());
        assert!(validate_gateway_url(&Url::parse("wss://gateway.example.test").unwrap()).is_ok());
    }

    #[test]
    fn workspace_must_exist_and_be_absolute() {
        assert!(canonical_workspaces(&[PathBuf::from("relative")]).is_err());
    }

    #[test]
    fn another_browser_cannot_claim_an_owned_thread() {
        let mut owners = HashMap::new();
        let first = remote_protocol::BrowserSessionId(Uuid::new_v4());
        let second = remote_protocol::BrowserSessionId(Uuid::new_v4());
        assert!(claim_thread_owner(&mut owners, "thread-a", &first));
        assert!(claim_thread_owner(&mut owners, "thread-a", &first));
        assert!(!claim_thread_owner(&mut owners, "thread-a", &second));
    }

    #[test]
    fn thread_start_must_use_a_registered_workspace() {
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let valid = json!({ "id": 1, "method": "thread/start", "params": { "cwd": root } });
        assert!(validate_workspace_message(&valid, std::slice::from_ref(&root)).is_ok());
        let invalid = json!({ "id": 2, "method": "thread/start", "params": { "cwd": root.parent().unwrap() } });
        assert!(validate_workspace_message(&invalid, &[root]).is_err());
    }
}
