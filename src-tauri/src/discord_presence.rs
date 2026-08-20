use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

const PRESENCE_FIELD_LIMIT: usize = 128;
const UPDATE_THROTTLE: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSettings {
    pub enabled: bool,
    pub show_task_name: bool,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            show_task_name: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PresenceKind {
    Disconnected,
    ConnectingPair,
    ConnectingQr,
    ConnectingAppServer,
    ConnectedIdle,
    Working,
    WaitingApproval,
    WaitingInput,
    ConnectionError,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresenceUpdate {
    pub generation: u64,
    pub kind: PresenceKind,
    #[serde(default)]
    pub task_name: Option<String>,
    #[serde(default)]
    pub has_selected_task: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceServiceInfo {
    pub configured: bool,
    pub enabled: bool,
    pub show_task_name: bool,
    pub connection_state: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresenceStatusEvent {
    configured: bool,
    state: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RenderedPresence {
    details: String,
    state: String,
}

#[derive(Default)]
struct PresenceModel {
    latest_generation: u64,
    latest: Option<PresenceUpdate>,
}

impl PresenceModel {
    fn accept(&mut self, update: PresenceUpdate) -> bool {
        if update.generation < self.latest_generation {
            return false;
        }
        if update.generation == self.latest_generation && self.latest.as_ref() == Some(&update) {
            return false;
        }
        self.latest_generation = update.generation;
        self.latest = Some(update);
        true
    }
}

enum WorkerCommand {
    Update(PresenceUpdate),
    Settings(PresenceSettings),
    Shutdown(Sender<()>),
}

pub struct DiscordPresenceService {
    sender: Sender<WorkerCommand>,
    settings: Arc<Mutex<PresenceSettings>>,
    connection_state: Arc<Mutex<&'static str>>,
    configured: bool,
    stopped: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl DiscordPresenceService {
    pub fn start(app: AppHandle) -> Self {
        let settings = Arc::new(Mutex::new(load_settings(&app).unwrap_or_default()));
        let connection_state = Arc::new(Mutex::new("disabled"));
        let application_id = configured_application_id();
        let configured = application_id.is_some();
        let (sender, receiver) = mpsc::channel();
        let worker_settings = Arc::clone(&settings);
        let worker_state = Arc::clone(&connection_state);
        let worker = thread::Builder::new()
            .name("discord-presence".to_string())
            .spawn(move || run_worker(app, application_id, receiver, worker_settings, worker_state))
            .ok();
        Self {
            sender,
            settings,
            connection_state,
            configured,
            stopped: AtomicBool::new(false),
            worker: Mutex::new(worker),
        }
    }

    pub fn update(&self, update: PresenceUpdate) {
        let _ = self.sender.send(WorkerCommand::Update(update));
    }

    pub fn set_settings(&self, app: &AppHandle, settings: PresenceSettings) -> Result<(), String> {
        save_settings(app, &settings)?;
        *self
            .settings
            .lock()
            .map_err(|_| "Discord設定を更新できません".to_string())? = settings.clone();
        let _ = self.sender.send(WorkerCommand::Settings(settings));
        Ok(())
    }

    pub fn info(&self) -> PresenceServiceInfo {
        let settings = self
            .settings
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let connection_state = self
            .connection_state
            .lock()
            .map(|value| *value)
            .unwrap_or("disconnected");
        PresenceServiceInfo {
            configured: self.configured,
            enabled: settings.enabled,
            show_task_name: settings.show_task_name,
            connection_state,
        }
    }

    pub fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let (ack_sender, ack_receiver) = mpsc::channel();
        let _ = self.sender.send(WorkerCommand::Shutdown(ack_sender));
        let acknowledged = ack_receiver.recv_timeout(Duration::from_secs(2)).is_ok();
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                if acknowledged {
                    let _ = handle.join();
                }
            }
        }
    }
}

impl Drop for DiscordPresenceService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_worker(
    app: AppHandle,
    application_id: Option<String>,
    receiver: Receiver<WorkerCommand>,
    settings: Arc<Mutex<PresenceSettings>>,
    connection_state: Arc<Mutex<&'static str>>,
) {
    let mut current_settings = settings
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let mut model = PresenceModel::default();
    let mut client: Option<DiscordIpcClient> = None;
    let mut last_sent: Option<RenderedPresence> = None;
    let mut last_send_at: Option<Instant> = None;
    let mut next_retry_at = Instant::now();
    let mut retry_attempt = 0_u32;

    set_status(
        &app,
        &connection_state,
        application_id.is_some(),
        if application_id.is_none() || !current_settings.enabled {
            "disabled"
        } else {
            "disconnected"
        },
    );

    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(WorkerCommand::Update(update)) => {
                model.accept(update);
            }
            Ok(WorkerCommand::Settings(next)) => {
                current_settings = next;
                last_sent = None;
                if !current_settings.enabled {
                    if let Some(mut connected) = client.take() {
                        let _ = connected.clear_activity();
                        let _ = connected.close();
                    }
                    set_status(
                        &app,
                        &connection_state,
                        application_id.is_some(),
                        "disabled",
                    );
                } else {
                    next_retry_at = Instant::now();
                    retry_attempt = 0;
                }
            }
            Ok(WorkerCommand::Shutdown(ack)) => {
                if let Some(mut connected) = client.take() {
                    let _ = connected.clear_activity();
                    let _ = connected.close();
                }
                let _ = ack.send(());
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let Some(application_id) = application_id.as_deref() else {
            continue;
        };
        if !current_settings.enabled {
            continue;
        }
        let Some(update) = model.latest.as_ref() else {
            continue;
        };
        let rendered = render_presence(update, &current_settings);
        let now = Instant::now();

        if client.is_none() {
            if now < next_retry_at {
                continue;
            }
            set_status(&app, &connection_state, true, "connecting");
            let mut candidate = DiscordIpcClient::new(application_id);
            match candidate.connect() {
                Ok(()) => {
                    client = Some(candidate);
                    retry_attempt = 0;
                    last_sent = None;
                    set_status(&app, &connection_state, true, "connected");
                }
                Err(_) => {
                    set_status(&app, &connection_state, true, "disconnected");
                    next_retry_at = now + reconnect_backoff(retry_attempt);
                    retry_attempt = retry_attempt.saturating_add(1);
                    continue;
                }
            }
        }

        let throttled =
            last_send_at.is_some_and(|sent_at| now.duration_since(sent_at) < UPDATE_THROTTLE);
        let heartbeat_due =
            last_send_at.is_none_or(|sent_at| now.duration_since(sent_at) >= HEALTH_INTERVAL);
        let changed = last_sent.as_ref() != Some(&rendered);
        if throttled || (!changed && !heartbeat_due) {
            continue;
        }

        let result = client.as_mut().map(|connected| {
            connected.set_activity(
                activity::Activity::new()
                    .details(rendered.details.as_str())
                    .state(rendered.state.as_str()),
            )
        });
        match result {
            Some(Ok(())) => {
                last_sent = Some(rendered);
                last_send_at = Some(now);
                retry_attempt = 0;
                set_status(&app, &connection_state, true, "connected");
            }
            _ => {
                if let Some(mut failed) = client.take() {
                    let _ = failed.close();
                }
                set_status(&app, &connection_state, true, "disconnected");
                next_retry_at = now + reconnect_backoff(retry_attempt);
                retry_attempt = retry_attempt.saturating_add(1);
            }
        }
    }
}

fn render_presence(update: &PresenceUpdate, settings: &PresenceSettings) -> RenderedPresence {
    let (details, state) = match update.kind {
        PresenceKind::Disconnected => ("Codex Remote", "接続待ち"),
        PresenceKind::ConnectingPair => ("Codexへ接続中", "Pair接続中"),
        PresenceKind::ConnectingQr => ("Codexへ接続中", "QR Pair接続中"),
        PresenceKind::ConnectingAppServer => ("Codexへ接続中", "App Serverへ接続中"),
        PresenceKind::ConnectedIdle => (
            "Codex Remote",
            if update.has_selected_task {
                "タスクを選択中"
            } else {
                "待機中"
            },
        ),
        PresenceKind::Working => (
            "Codexで作業中",
            if settings.show_task_name {
                update.task_name.as_deref().unwrap_or("Codexで作業中")
            } else {
                "Codexで作業中"
            },
        ),
        PresenceKind::WaitingApproval => ("Codexが操作を待っています", "承認待ち"),
        PresenceKind::WaitingInput => ("Codexが操作を待っています", "入力待ち"),
        PresenceKind::ConnectionError => ("Codex Remote", "接続エラー"),
    };
    RenderedPresence {
        details: sanitize_field(details, "Codex Remote"),
        state: sanitize_field(state, "Codexで作業中"),
    }
}

fn sanitize_field(value: &str, fallback: &str) -> String {
    let cleaned = value
        .chars()
        .map(|character| {
            if character.is_control() || character == '\n' || character == '\r' {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let source = if cleaned.chars().count() >= 2 {
        cleaned.as_str()
    } else {
        fallback
    };
    source.chars().take(PRESENCE_FIELD_LIMIT).collect()
}

fn reconnect_backoff(attempt: u32) -> Duration {
    Duration::from_secs((1_u64 << attempt.min(6)).min(MAX_BACKOFF.as_secs()))
}

fn configured_application_id() -> Option<String> {
    std::env::var("CODEX_REMOTE_DISCORD_APP_ID")
        .ok()
        .or_else(|| option_env!("CODEX_REMOTE_DISCORD_APP_ID").map(ToString::to_string))
        .map(|value| value.trim().to_string())
        .filter(|value| {
            (17..=20).contains(&value.len())
                && value.chars().all(|character| character.is_ascii_digit())
        })
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Discord設定の保存先を取得できません: {error}"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Discord設定の保存先を作成できません: {error}"))?;
    Ok(directory.join("discord-presence.json"))
}

fn load_settings(app: &AppHandle) -> Result<PresenceSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(PresenceSettings::default());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("Discord設定を読み取れません: {error}"))?;
    serde_json::from_str(&contents).map_err(|error| format!("Discord設定を解析できません: {error}"))
}

fn save_settings(app: &AppHandle, settings: &PresenceSettings) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("Discord設定を保存できません: {error}"))?;
    fs::write(settings_path(app)?, contents)
        .map_err(|error| format!("Discord設定を保存できません: {error}"))
}

fn set_status(
    app: &AppHandle,
    state: &Arc<Mutex<&'static str>>,
    configured: bool,
    next: &'static str,
) {
    let changed = state
        .lock()
        .map(|mut current| {
            if *current == next {
                false
            } else {
                *current = next;
                true
            }
        })
        .unwrap_or(false);
    if changed {
        let _ = app.emit(
            "discord-presence-status",
            PresenceStatusEvent {
                configured,
                state: next,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(generation: u64, kind: PresenceKind, task_name: Option<&str>) -> PresenceUpdate {
        PresenceUpdate {
            generation,
            kind,
            task_name: task_name.map(ToString::to_string),
            has_selected_task: false,
        }
    }

    #[test]
    fn suppresses_duplicate_and_stale_updates() {
        let mut model = PresenceModel::default();
        let current = update(2, PresenceKind::ConnectedIdle, None);
        assert!(model.accept(current.clone()));
        assert!(!model.accept(current));
        assert!(!model.accept(update(1, PresenceKind::Working, Some("old"))));
        assert_eq!(
            model.latest.as_ref().unwrap().kind,
            PresenceKind::ConnectedIdle
        );
    }

    #[test]
    fn task_name_is_not_rendered_when_disabled() {
        let rendered = render_presence(
            &update(1, PresenceKind::Working, Some("secret repository")),
            &PresenceSettings {
                enabled: true,
                show_task_name: false,
            },
        );
        assert_eq!(rendered.state, "Codexで作業中");
        assert!(!rendered.state.contains("secret"));
    }

    #[test]
    fn task_name_is_sanitized_and_truncated() {
        let task_name = format!("line one\nline two\u{0000} {}", "長".repeat(200));
        let rendered = render_presence(
            &update(1, PresenceKind::Working, Some(&task_name)),
            &PresenceSettings {
                enabled: true,
                show_task_name: true,
            },
        );
        assert!(!rendered.state.contains('\n'));
        assert!(!rendered.state.chars().any(char::is_control));
        assert!(rendered.state.chars().count() <= PRESENCE_FIELD_LIMIT);
    }

    #[test]
    fn empty_task_name_uses_safe_fallback() {
        let rendered = render_presence(
            &update(1, PresenceKind::Working, Some("\n")),
            &PresenceSettings {
                enabled: true,
                show_task_name: true,
            },
        );
        assert_eq!(rendered.state, "Codexで作業中");
    }

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_backoff(0), Duration::from_secs(1));
        assert_eq!(reconnect_backoff(4), Duration::from_secs(16));
        assert_eq!(reconnect_backoff(20), MAX_BACKOFF);
    }

    #[test]
    fn disconnected_and_waiting_states_match_the_product_copy() {
        let settings = PresenceSettings::default();
        assert_eq!(
            render_presence(&update(1, PresenceKind::Disconnected, None), &settings),
            RenderedPresence {
                details: "Codex Remote".to_string(),
                state: "接続待ち".to_string()
            }
        );
        assert_eq!(
            render_presence(&update(2, PresenceKind::WaitingApproval, None), &settings).state,
            "承認待ち"
        );
    }
}
