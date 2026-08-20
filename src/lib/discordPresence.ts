import { invoke } from "@tauri-apps/api/core";

export type DiscordPresenceKind =
  | "disconnected"
  | "connectingPair"
  | "connectingQr"
  | "connectingAppServer"
  | "connectedIdle"
  | "working"
  | "waitingApproval"
  | "waitingInput"
  | "connectionError";

export type PresenceInputs = {
  connectionAdding: boolean;
  connectionMode: "manual" | "qr" | "advanced";
  connectionError: boolean;
  connected: boolean;
  busy: boolean;
  hasSelectedTask: boolean;
  taskName?: string | null;
  pendingMethod?: string | null;
};

export function deriveDiscordPresence(inputs: PresenceInputs) {
  let kind: DiscordPresenceKind;
  if (inputs.connectionAdding) {
    kind = inputs.connectionMode === "manual"
      ? "connectingPair"
      : inputs.connectionMode === "qr"
        ? "connectingQr"
        : "connectingAppServer";
  } else if (!inputs.connected && inputs.connectionError) {
    kind = "connectionError";
  } else if (!inputs.connected) {
    kind = "disconnected";
  } else if (inputs.pendingMethod) {
    kind = isInputRequest(inputs.pendingMethod) ? "waitingInput" : "waitingApproval";
  } else if (inputs.busy) {
    kind = "working";
  } else {
    kind = "connectedIdle";
  }
  return {
    kind,
    taskName: kind === "working" ? inputs.taskName || null : null,
    hasSelectedTask: inputs.hasSelectedTask,
  };
}

export async function updateDiscordPresence(update: {
  generation: number;
  kind: DiscordPresenceKind;
  taskName: string | null;
  hasSelectedTask: boolean;
}) {
  await invoke("discord_presence_update", { update });
}

function isInputRequest(method: string) {
  return method === "item/tool/requestUserInput"
    || method === "requestUserInput"
    || method.includes("request_user_input")
    || method.includes("requestUserInput")
    || method.includes("elicitation");
}
