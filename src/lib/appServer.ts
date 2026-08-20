import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConnectionStatus,
  ConnectionTiming,
  IncomingEnvelope,
  InitializeResponse,
  JsonId,
  PairingProgress,
  ServerRequest,
  WireMessage,
} from "../types";

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

type ClientHandlers = {
  onNotification: (message: WireMessage) => void;
  onServerRequest: (request: ServerRequest) => void;
  onStatus: (status: ConnectionStatus) => void;
  onPairingProgress?: (progress: PairingProgress) => void;
  onTiming?: (timing: ConnectionTiming) => void;
  onConnectionPhase?: (phase: "connecting" | "initializing") => void;
};

class AppServerEventHub {
  private clients = new Set<AppServerClient>();
  private mounting: Promise<void> | null = null;
  private unlisten: UnlistenFn[] = [];

  async add(client: AppServerClient) {
    this.clients.add(client);
    try {
      if (!this.mounting) this.mounting = this.install();
      await this.mounting;
    } catch (error) {
      this.clients.delete(client);
      throw error;
    }
  }

  remove(client: AppServerClient) {
    this.clients.delete(client);
    if (!this.clients.size) void this.teardownWhenIdle();
  }

  private async install() {
    const listeners = await Promise.allSettled([
      listen<IncomingEnvelope>("app-server-message", ({ payload }) => {
        for (const client of this.clients) client.dispatchMessage(payload);
      }),
      listen<ConnectionStatus>("app-server-status", ({ payload }) => {
        for (const client of this.clients) client.dispatchStatus(payload);
      }),
      listen<PairingProgress>("pairing-progress", ({ payload }) => {
        for (const client of this.clients) client.dispatchPairingProgress(payload);
      }),
      listen<ConnectionTiming>("connection-timing", ({ payload }) => {
        for (const client of this.clients) client.dispatchTiming(payload);
      }),
    ]);
    const installed = listeners
      .filter((result): result is PromiseFulfilledResult<UnlistenFn> => result.status === "fulfilled")
      .map((result) => result.value);
    const failed = listeners.find((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failed) {
      for (const unlisten of installed) unlisten();
      this.mounting = null;
      throw failed.reason;
    }
    this.unlisten = installed;
  }

  private async teardownWhenIdle() {
    await this.mounting?.catch(() => undefined);
    if (this.clients.size) return;
    for (const unlisten of this.unlisten) unlisten();
    this.unlisten = [];
    this.mounting = null;
  }
}

const eventHub = new AppServerEventHub();

export class AppServerClient {
  private nextId = 1;
  private connectionId = 0;
  private pending = new Map<JsonId, PendingRequest>();
  private mounted = false;
  private disposed = false;
  private connectGeneration = 0;
  private activeAttemptId: string | null = null;
  private lastAttemptId: string | null = null;
  private attemptStartedAt = 0;

  constructor(private readonly handlers: ClientHandlers) {}

  async mount() {
    if (this.mounted) return;
    this.disposed = false;
    await eventHub.add(this);
    this.mounted = true;
  }

  async connect(url: string, bearerToken?: string): Promise<InitializeResponse> {
    return this.beginConnection("connect_app_server", {
      url,
      bearerToken: bearerToken || null,
    });
  }

  async connectPair(code: string, kind: "manual" | "qr"): Promise<InitializeResponse> {
    return this.beginConnection("connect_paired_app_server", {
      request: { code, kind },
    });
  }

  async preparePair(): Promise<void> {
    const previousAttemptId = this.activeAttemptId;
    if (previousAttemptId) {
      await invoke("cancel_connection_attempt", { attemptId: previousAttemptId }).catch(() => undefined);
    }
    const generation = ++this.connectGeneration;
    const attemptId = createAttemptId();
    this.activeAttemptId = attemptId;
    this.lastAttemptId = attemptId;
    this.attemptStartedAt = performance.now();
    this.handlers.onConnectionPhase?.("connecting");
    try {
      await invoke("prepare_pair_connection", { attemptId });
      if (!this.isCurrentAttempt(generation, attemptId)) {
        throw new Error("古いPair準備処理を破棄しました");
      }
    } finally {
      if (this.activeAttemptId === attemptId) this.activeAttemptId = null;
    }
  }

  private async beginConnection(
    command: "connect_app_server" | "connect_paired_app_server",
    args: Record<string, unknown>,
  ) {
    const previousAttemptId = this.activeAttemptId;
    if (previousAttemptId) {
      void invoke("cancel_connection_attempt", { attemptId: previousAttemptId }).catch(() => undefined);
      this.rejectAll("新しい接続試行を開始しました");
      const previousConnectionId = this.connectionId;
      this.connectionId = 0;
      if (previousConnectionId) {
        void invoke("disconnect_app_server", { connectionId: previousConnectionId }).catch(() => undefined);
      }
    }
    const generation = ++this.connectGeneration;
    const attemptId = createAttemptId();
    this.activeAttemptId = attemptId;
    this.lastAttemptId = attemptId;
    this.attemptStartedAt = performance.now();
    this.handlers.onConnectionPhase?.("connecting");
    try {
      const info = await invoke<{ connectionId: number }>(command, { ...args, attemptId });
      if (!this.isCurrentAttempt(generation, attemptId)) {
        await invoke("disconnect_app_server", { connectionId: info.connectionId }).catch(() => undefined);
        throw new Error("古い接続試行を破棄しました");
      }
      this.handlers.onConnectionPhase?.("initializing");
      return await this.initialize(info.connectionId, generation, attemptId);
    } finally {
      if (this.activeAttemptId === attemptId) this.activeAttemptId = null;
    }
  }

  private async initialize(
    connectionId: number,
    generation: number,
    attemptId: string,
  ): Promise<InitializeResponse> {
    this.connectionId = connectionId;
    try {
      const initialized = await this.request<InitializeResponse>("initialize", {
        clientInfo: {
          name: "codex_remote_tauri",
          title: "Codex Remote",
          version: "0.1.0",
        },
        capabilities: {
          experimentalApi: true,
        },
      });
      this.recordTiming(attemptId, "initialize_request_completed");
      if (!this.isCurrentAttempt(generation, attemptId)) {
        throw new Error("古い接続試行を破棄しました");
      }
      await this.notify("initialized", {});
      this.recordTiming(attemptId, "initialized_notification_sent");
      if (!this.isCurrentAttempt(generation, attemptId)) {
        throw new Error("古い接続試行を破棄しました");
      }
      return initialized;
    } catch (error) {
      await invoke("disconnect_app_server", { connectionId }).catch(() => undefined);
      if (this.connectionId === connectionId) this.connectionId = 0;
      throw error;
    }
  }

  async disconnect() {
    this.cancelPendingConnect();
    this.rejectAll("切断しました");
    if (this.connectionId) {
      await invoke("disconnect_app_server", { connectionId: this.connectionId });
    }
  }

  async cancelConnectionAttempt() {
    const attemptId = this.activeAttemptId;
    this.connectGeneration += 1;
    this.activeAttemptId = null;
    this.rejectAll("接続試行をキャンセルしました");
    if (attemptId) {
      await invoke("cancel_connection_attempt", { attemptId });
    }
  }

  getConnectionId() {
    return this.connectionId;
  }

  getLastAttemptId() {
    return this.lastAttemptId;
  }

  recordPhase(phase: string) {
    if (this.lastAttemptId) this.recordTiming(this.lastAttemptId, phase);
  }

  async request<T>(method: string, params?: unknown, timeoutMs = 30_000): Promise<T> {
    for (let attempt = 0; ; attempt += 1) {
      try {
        return await this.requestOnce<T>(method, params, timeoutMs);
      } catch (error) {
        const overloaded = /-32001|overloaded|retry later/i.test(String(error));
        if (!overloaded || attempt >= 3) throw error;
        const delayMs = 250 * 2 ** attempt + Math.floor(Math.random() * 180);
        await new Promise((resolve) => setTimeout(resolve, delayMs));
      }
    }
  }

  private async requestOnce<T>(method: string, params?: unknown, timeoutMs = 30_000): Promise<T> {
    const id = this.nextId++;
    const message: WireMessage = { method, id };
    if (params !== undefined) message.params = params as Record<string, unknown>;

    const response = new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} がタイムアウトしました`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
        timer,
      });
    });

    try {
      await invoke("send_app_server_message", { connectionId: this.connectionId, message });
    } catch (error) {
      const pending = this.pending.get(id);
      if (pending) {
        clearTimeout(pending.timer);
        this.pending.delete(id);
        pending.reject(toError(error));
      }
    }
    return response;
  }

  async notify(method: string, params?: unknown) {
    const message: WireMessage = { method };
    if (params !== undefined) message.params = params as Record<string, unknown>;
    await invoke("send_app_server_message", { connectionId: this.connectionId, message });
  }

  async respond(id: JsonId, result: unknown) {
    await invoke("send_app_server_message", {
      connectionId: this.connectionId,
      message: { id, result },
    });
  }

  async respondError(id: JsonId, code: number, message: string) {
    await invoke("send_app_server_message", {
      connectionId: this.connectionId,
      message: { id, error: { code, message } },
    });
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.cancelPendingConnect();
    this.rejectAll("クライアントを終了しました");
    if (this.mounted) eventHub.remove(this);
    this.mounted = false;
  }

  dispatchMessage(payload: IncomingEnvelope) {
    if (payload.connectionId === this.connectionId) this.receive(payload.message);
  }

  dispatchStatus(payload: ConnectionStatus) {
    if (payload.connectionId !== this.connectionId) return;
    this.handlers.onStatus(payload);
    if (payload.state !== "connected") {
      this.rejectAll(payload.detail || "App Server との接続が切断されました");
    }
  }

  dispatchPairingProgress(payload: PairingProgress) {
    if (payload.attemptId === this.activeAttemptId) this.handlers.onPairingProgress?.(payload);
  }

  dispatchTiming(payload: ConnectionTiming) {
    if (payload.attemptId === this.activeAttemptId || payload.attemptId === this.lastAttemptId) {
      this.handlers.onTiming?.(payload);
    }
  }

  private isCurrentAttempt(generation: number, attemptId: string) {
    return !this.disposed
      && generation === this.connectGeneration
      && attemptId === this.activeAttemptId;
  }

  private cancelPendingConnect() {
    this.connectGeneration += 1;
    const attemptId = this.activeAttemptId;
    this.activeAttemptId = null;
    if (attemptId) {
      void invoke("cancel_connection_attempt", { attemptId }).catch(() => undefined);
    }
  }

  private recordTiming(attemptId: string, phase: string) {
    const elapsedMs = Math.max(0, performance.now() - this.attemptStartedAt);
    void invoke("record_connection_timing", {
      timing: { attemptId, phase, elapsedMs },
    }).catch(() => undefined);
  }

  private receive(message: WireMessage) {
    if (message.id !== undefined && !message.method) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      clearTimeout(pending.timer);
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(
          new Error(
            `${message.error.message}${message.error.code ? ` (${message.error.code})` : ""}`,
          ),
        );
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (message.id !== undefined && message.method) {
      this.handlers.onServerRequest(message as ServerRequest);
      return;
    }

    if (message.method) this.handlers.onNotification(message);
  }

  private rejectAll(message: string) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error(message));
    }
    this.pending.clear();
  }
}

function toError(value: unknown) {
  return value instanceof Error ? value : new Error(String(value));
}

function createAttemptId() {
  return typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
