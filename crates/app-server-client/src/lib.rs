use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{broadcast, oneshot, Mutex},
    task::JoinHandle,
    time::timeout,
};

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_JSON_LINE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum AppServerError {
    #[error("failed to start codex app-server: {0}")]
    Spawn(std::io::Error),
    #[error("app-server stdin is unavailable")]
    MissingStdin,
    #[error("app-server stdout is unavailable")]
    MissingStdout,
    #[error("app-server request timed out")]
    Timeout,
    #[error("app-server returned an error: {0}")]
    Rpc(String),
    #[error("app-server transport closed")]
    Closed,
    #[error("invalid app-server response: {0}")]
    InvalidResponse(String),
    #[error("failed to write to app-server: {0}")]
    Write(std::io::Error),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, AppServerError>>>>>;

pub struct AppServerProcess {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    notifications: broadcast::Sender<Value>,
    reader_task: JoinHandle<()>,
    next_id: std::sync::atomic::AtomicU64,
}

impl AppServerProcess {
    pub async fn spawn() -> Result<Self, AppServerError> {
        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(AppServerError::Spawn)?;
        let stdin = child.stdin.take().ok_or(AppServerError::MissingStdin)?;
        let stdout = child.stdout.take().ok_or(AppServerError::MissingStdout)?;
        let pending = Pending::default();
        let (notifications, _) = broadcast::channel(512);
        let reader_task = tokio::spawn(read_messages(
            stdout,
            pending.clone(),
            notifications.clone(),
        ));
        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            notifications,
            reader_task,
            next_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.notifications.subscribe()
    }

    pub async fn initialize(&self) -> Result<Value, AppServerError> {
        let response = self
            .request_value(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "hutoncodex_host_agent",
                        "title": "hutoncodex Host Agent",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "experimentalApi": true,
                        "requestAttestation": false
                    }
                }),
            )
            .await?;
        self.notify("initialized", None).await?;
        Ok(response)
    }

    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, AppServerError> {
        let value = self.request_value(method, params).await?;
        serde_json::from_value(value)
            .map_err(|error| AppServerError::InvalidResponse(error.to_string()))
    }

    pub async fn request_value(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, AppServerError> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self
            .write_json(&json!({ "id": id, "method": method, "params": params }))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match timeout(DEFAULT_REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AppServerError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(AppServerError::Timeout)
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), AppServerError> {
        let mut value = json!({ "method": method });
        if let Some(params) = params {
            value["params"] = params;
        }
        self.write_json(&value).await
    }

    pub async fn send_raw(&self, message: &Value) -> Result<(), AppServerError> {
        self.write_json(message).await
    }

    async fn write_json(&self, message: &Value) -> Result<(), AppServerError> {
        let mut encoded = serde_json::to_vec(message)
            .map_err(|error| AppServerError::InvalidResponse(error.to_string()))?;
        if encoded.len() > MAX_JSON_LINE_BYTES {
            return Err(AppServerError::InvalidResponse(
                "outgoing JSON-RPC message exceeds the size limit".to_string(),
            ));
        }
        encoded.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&encoded)
            .await
            .map_err(AppServerError::Write)?;
        stdin.flush().await.map_err(AppServerError::Write)
    }

    pub async fn shutdown(&self) -> Result<(), AppServerError> {
        self.reader_task.abort();
        let mut child = self.child.lock().await;
        if let Some(child) = child.as_mut() {
            child.kill().await.map_err(AppServerError::Write)?;
            let _ = child.wait().await;
        }
        *child = None;
        let mut pending = self.pending.lock().await;
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(AppServerError::Closed));
        }
        Ok(())
    }
}

async fn read_messages<R>(reader: R, pending: Pending, notifications: broadcast::Sender<Value>)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    while let Ok(Some(line)) = read_bounded_line(&mut reader).await {
        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            if message.get("method").is_none() {
                if let Some(sender) = pending.lock().await.remove(&id) {
                    let result = if let Some(error) = message.get("error") {
                        Err(AppServerError::Rpc(
                            error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown JSON-RPC error")
                                .to_string(),
                        ))
                    } else {
                        Ok(message.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = sender.send(result);
                    continue;
                }
            }
        }
        let _ = notifications.send(message);
    }
    let mut pending = pending.lock().await;
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(AppServerError::Closed));
    }
}

async fn read_bounded_line<R>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_JSON_LINE_BYTES + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "app-server JSON line exceeds the size limit",
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncWriteExt};

    #[tokio::test]
    async fn response_is_routed_to_its_request_id() {
        let (mut writer, reader) = duplex(2048);
        let pending = Pending::default();
        let (notifications, _) = broadcast::channel(4);
        let (sender, receiver) = oneshot::channel();
        pending.lock().await.insert(7, sender);
        let task = tokio::spawn(read_messages(reader, pending, notifications));
        writer
            .write_all(b"{\"id\":7,\"result\":{\"ok\":true}}\n")
            .await
            .unwrap();
        assert_eq!(receiver.await.unwrap().unwrap(), json!({ "ok": true }));
        task.abort();
    }

    #[tokio::test]
    async fn server_requests_are_not_consumed_as_responses() {
        let (mut writer, reader) = duplex(2048);
        let pending = Pending::default();
        let (notifications, mut subscriber) = broadcast::channel(4);
        let task = tokio::spawn(read_messages(reader, pending, notifications));
        writer
            .write_all(b"{\"id\":4,\"method\":\"item/tool/requestUserInput\",\"params\":{}}\n")
            .await
            .unwrap();
        let message = subscriber.recv().await.unwrap();
        assert_eq!(message["method"], "item/tool/requestUserInput");
        task.abort();
    }

    #[tokio::test]
    async fn bounded_reader_rejects_an_oversized_line_before_a_newline() {
        let payload = vec![b'a'; MAX_JSON_LINE_BYTES + 2];
        let mut reader = BufReader::new(payload.as_slice());
        let error = read_bounded_line(&mut reader).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
