import { invoke } from "@tauri-apps/api/core";

export type RuntimeCapabilities = {
  mobile: boolean;
  pairingSupported: boolean;
  discordPresenceSupported: boolean;
};

const MOBILE_USER_AGENT = /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);

export const DEFAULT_RUNTIME_CAPABILITIES: RuntimeCapabilities = {
  mobile: MOBILE_USER_AGENT,
  pairingSupported: !MOBILE_USER_AGENT,
  discordPresenceSupported: !MOBILE_USER_AGENT,
};

export async function getRuntimeCapabilities(): Promise<RuntimeCapabilities> {
  if (!("__TAURI_INTERNALS__" in window)) return DEFAULT_RUNTIME_CAPABILITIES;
  return invoke<RuntimeCapabilities>("runtime_capabilities");
}
