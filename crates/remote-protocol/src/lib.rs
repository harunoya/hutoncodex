use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const GATEWAY_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UserId(pub Uuid);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HostId(pub Uuid);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BrowserSessionId(pub Uuid);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SubagentJobId(pub Uuid);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppServerEnvelope {
    pub host_id: HostId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_session_id: Option<BrowserSessionId>,
    pub app_server_process_id: Uuid,
    pub connection_generation: u64,
    pub sequence: u64,
    pub message: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentToGateway {
    Hello {
        protocol_version: u32,
        host_id: HostId,
        generation: u64,
        display_name: String,
    },
    AppServerMessage {
        envelope: AppServerEnvelope,
    },
    Status {
        host_id: HostId,
        generation: u64,
        state: HostConnectionState,
        detail: Option<String>,
    },
    Capabilities {
        host_id: HostId,
        generation: u64,
        luna_max: LunaMaxCapability,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GatewayToAgent {
    AppServerMessage {
        browser_session_id: BrowserSessionId,
        host_id: HostId,
        generation: u64,
        message: Value,
    },
    Shutdown {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostConnectionState {
    Connecting,
    Connected,
    AppServerStarting,
    AppServerReady,
    Disconnected,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub model: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<ReasoningEffortOption>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortOption {
    pub reasoning_effort: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LunaMaxCapability {
    Available { model: String, effort: String },
    Unavailable { reason: String },
}

pub fn detect_luna_max(models: &[CatalogModel]) -> LunaMaxCapability {
    let Some(model) = models.iter().find(|entry| entry.model == "gpt-5.6-luna") else {
        return LunaMaxCapability::Unavailable {
            reason: "model/list に gpt-5.6-luna がありません".to_string(),
        };
    };
    if !model
        .supported_reasoning_efforts
        .iter()
        .any(|entry| entry.reasoning_effort == "max")
    {
        return LunaMaxCapability::Unavailable {
            reason: "gpt-5.6-luna で max reasoning effort を利用できません".to_string(),
        };
    }
    LunaMaxCapability::Available {
        model: model.model.clone(),
        effort: "max".to_string(),
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentJob {
    pub id: SubagentJobId,
    pub owner_user_id: UserId,
    pub host_id: HostId,
    pub parent_thread_id: String,
    pub child_thread_id: Option<String>,
    pub model: String,
    pub reasoning_effort: String,
    pub task: String,
    pub status: SubagentJobStatus,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    UnknownAfterDisconnect,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luna_max_requires_exact_model_and_effort() {
        let models = vec![CatalogModel {
            model: "gpt-5.6-luna".to_string(),
            display_name: "GPT-5.6 Luna".to_string(),
            supported_reasoning_efforts: vec![ReasoningEffortOption {
                reasoning_effort: "max".to_string(),
            }],
        }];
        assert!(matches!(
            detect_luna_max(&models),
            LunaMaxCapability::Available { .. }
        ));
    }

    #[test]
    fn luna_max_does_not_fallback_to_another_model() {
        let models = vec![CatalogModel {
            model: "gpt-5.6-terra".to_string(),
            display_name: String::new(),
            supported_reasoning_efforts: vec![ReasoningEffortOption {
                reasoning_effort: "max".to_string(),
            }],
        }];
        assert!(matches!(
            detect_luna_max(&models),
            LunaMaxCapability::Unavailable { .. }
        ));
    }
}
