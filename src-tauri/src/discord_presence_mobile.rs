use serde::{Deserialize, Serialize};
use tauri::AppHandle;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSettings {
    pub enabled: bool,
    pub show_task_name: bool,
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

pub struct DiscordPresenceService;

impl DiscordPresenceService {
    pub fn start(_app: AppHandle) -> Self {
        Self
    }

    pub fn update(&self, _update: PresenceUpdate) {}

    pub fn set_settings(
        &self,
        _app: &AppHandle,
        _settings: PresenceSettings,
    ) -> Result<(), String> {
        Ok(())
    }

    pub fn info(&self) -> PresenceServiceInfo {
        PresenceServiceInfo {
            configured: false,
            enabled: false,
            show_task_name: false,
            connection_state: "disabled",
        }
    }

    pub fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_presence_reports_disabled() {
        let info = DiscordPresenceService.info();
        assert!(!info.configured);
        assert!(!info.enabled);
        assert_eq!(info.connection_state, "disabled");
    }
}
