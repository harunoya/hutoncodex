import type { JsonId, ServerRequest, WireMessage } from "../types";

export type GatewayHost = {
  id: string;
  displayName: string;
  generation: number;
  state: "appServerReady";
  lunaMax?:
    | { state: "available"; model: "gpt-5.6-luna"; effort: "max" }
    | { state: "unavailable"; reason: string };
};

export type GatewaySessionInfo = {
  userId: string;
  csrfToken: string;
};

export type GatewayClientHandlers = {
  onNotification: (message: WireMessage) => void;
  onServerRequest: (request: ServerRequest) => void;
  onStatus: (state: string, detail?: string) => void;
  onResyncRequired?: () => void;
  onCapabilities?: (capability: GatewayHost["lunaMax"]) => void;
};

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

type AgentEvent =
  | {
      type: "appServerMessage";
      envelope: {
        hostId: string;
        browserSessionId?: string;
        connectionGeneration: number;
        sequence: number;
        message: WireMessage;
      };
    }
  | {
      type: "status";
      hostId: string;
      generation: number;
      state: string;
      detail?: string;
    }
  | {
      type: "capabilities";
      hostId: string;
      generation: number;
      lunaMax:
        | { state: "available"; model: "gpt-5.6-luna"; effort: "max" }
        | { state: "unavailable"; reason: string };
    }
  | { type: "resyncRequired" };

/**
 * Browser-side BFF client. It retains only the opaque HttpOnly cookie and an
 * in-memory CSRF token; Codex credentials and Host Agent credentials never
 * enter this class or browser storage.
 */
export class GatewaySession {
  private csrfToken = "";

  constructor(private readonly baseUrl = window.location.origin) {}

  async login(password: string): Promise<GatewaySessionInfo> {
    const response = await this.fetchJson<GatewaySessionInfo>("/api/v1/session/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password }),
    });
    this.csrfToken = response.csrfToken;
    return response;
  }

  async restore(): Promise<GatewaySessionInfo> {
    const session = await this.fetchJson<GatewaySessionInfo>("/api/v1/session/me");
    this.csrfToken = session.csrfToken;
    return session;
  }

  async logout(): Promise<void> {
    await this.fetchJson("/api/v1/session/logout", this.mutation());
    this.csrfToken = "";
  }

  async listHosts(): Promise<GatewayHost[]> {
    const response = await this.fetchJson<{ data: GatewayHost[] }>("/api/v1/hosts");
    return response.data;
  }

  connection(host: GatewayHost, handlers: GatewayClientHandlers) {
    return new GatewayAppServerClient(this, host, handlers);
  }

  async send(host: GatewayHost, message: WireMessage): Promise<void> {
    await this.fetchJson(`/api/v1/hosts/${encodeURIComponent(host.id)}/rpc`, {
      ...this.mutation(),
      body: JSON.stringify({ generation: host.generation, message }),
    });
  }

  async openEvents(onEvent: (event: AgentEvent) => void): Promise<WebSocket> {
    const ticket = await this.fetchJson<{
      ticket: string;
      protocol: string;
    }>("/api/v1/events/ticket", this.mutation());
    const url = new URL("/api/v1/events", this.baseUrl);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url, [ticket.protocol, ticket.ticket]);
    socket.addEventListener("message", (event) => {
      if (typeof event.data !== "string" || event.data.length > 1_048_576) return;
      const parsed = parseAgentEvent(event.data);
      if (parsed) onEvent(parsed);
    });
    return socket;
  }

  private mutation(): RequestInit {
    if (!this.csrfToken) throw new Error("認証済みセッションがありません");
    return {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-csrf-token": this.csrfToken,
      },
    };
  }

  private async fetchJson<T = unknown>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(new URL(path, this.baseUrl), {
      ...init,
      credentials: "same-origin",
      redirect: "error",
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => null)) as {
        error?: { message?: string };
      } | null;
      throw new Error(body?.error?.message || `Gateway request failed (${response.status})`);
    }
    if (response.status === 204) return undefined as T;
    const text = await response.text();
    return (text ? JSON.parse(text) : undefined) as T;
  }
}

export class GatewayAppServerClient {
  private nextId = 1;
  private pending = new Map<JsonId, PendingRequest>();
  private socket: WebSocket | null = null;
  private lastSequence = 0;

  constructor(
    private readonly session: GatewaySession,
    private readonly host: GatewayHost,
    private readonly handlers: GatewayClientHandlers,
  ) {}

  async connect(): Promise<void> {
    if (this.socket) return;
    this.socket = await this.session.openEvents((event) => this.receiveEvent(event));
    await new Promise<void>((resolve, reject) => {
      const socket = this.socket!;
      const timer = setTimeout(() => reject(new Error("Gateway event接続がタイムアウトしました")), 10_000);
      socket.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      }, { once: true });
      socket.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error("Gateway event接続に失敗しました"));
      }, { once: true });
    });
    this.socket.addEventListener("close", () => {
      this.socket = null;
      for (const [id] of this.pending) this.rejectPending(id, new Error("Gateway event接続が切断されました"));
      this.handlers.onStatus("disconnected");
    });
    this.handlers.onStatus("connected");
  }

  async request<T>(method: string, params?: unknown, timeoutMs = 30_000): Promise<T> {
    const id = this.nextId++;
    const message: WireMessage = { id, method };
    if (params !== undefined) message.params = params as Record<string, unknown>;
    const response = new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} がタイムアウトしました`));
      }, timeoutMs);
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
    });
    try {
      await this.session.send(this.host, message);
    } catch (error) {
      this.rejectPending(id, toError(error));
    }
    return response;
  }

  async notify(method: string, params?: unknown): Promise<void> {
    const message: WireMessage = { method };
    if (params !== undefined) message.params = params as Record<string, unknown>;
    await this.session.send(this.host, message);
  }

  async respond(id: JsonId, result: unknown): Promise<void> {
    await this.session.send(this.host, { id, result });
  }

  async respondError(id: JsonId, code: number, message: string): Promise<void> {
    await this.session.send(this.host, { id, error: { code, message } });
  }

  disconnect(): void {
    this.socket?.close();
    this.socket = null;
    for (const [id] of this.pending) this.rejectPending(id, new Error("切断しました"));
    this.handlers.onStatus("disconnected");
  }

  private receiveEvent(event: AgentEvent) {
    if (event.type === "resyncRequired") {
      this.handlers.onResyncRequired?.();
      return;
    }
    if (event.type === "status") {
      if (event.hostId !== this.host.id || event.generation !== this.host.generation) return;
      this.handlers.onStatus(event.state, event.detail);
      return;
    }
    if (event.type === "capabilities") {
      if (event.hostId === this.host.id && event.generation === this.host.generation) {
        this.handlers.onCapabilities?.(event.lunaMax);
      }
      return;
    }
    if (event.envelope.hostId !== this.host.id
      || event.envelope.connectionGeneration !== this.host.generation) return;
    if (event.envelope.sequence <= this.lastSequence) return;
    if (this.lastSequence && event.envelope.sequence !== this.lastSequence + 1) {
      this.handlers.onResyncRequired?.();
    }
    this.lastSequence = event.envelope.sequence;
    this.receiveMessage(event.envelope.message);
  }

  private receiveMessage(message: WireMessage) {
    if (message.id !== undefined && !message.method) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      clearTimeout(pending.timer);
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
      return;
    }
    if (message.id !== undefined && message.method) {
      this.handlers.onServerRequest(message as ServerRequest);
    } else if (message.method) {
      this.handlers.onNotification(message);
    }
  }

  private rejectPending(id: JsonId, error: Error) {
    const pending = this.pending.get(id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(id);
    pending.reject(error);
  }
}

export function parseAgentEvent(text: string): AgentEvent | null {
  try {
    const value = JSON.parse(text) as AgentEvent;
    if (!value || typeof value !== "object" || typeof value.type !== "string") return null;
    if (value.type === "resyncRequired") return value;
    if (value.type === "status" || value.type === "capabilities") {
      return typeof value.hostId === "string" && Number.isSafeInteger(value.generation) ? value : null;
    }
    if (value.type === "appServerMessage") {
      return typeof value.envelope?.hostId === "string"
        && Number.isSafeInteger(value.envelope.connectionGeneration)
        && Number.isSafeInteger(value.envelope.sequence)
        && typeof value.envelope.message === "object"
        && value.envelope.message !== null
        ? value
        : null;
    }
  } catch {
    // Malformed or oversized frames are ignored and never reach product state.
  }
  return null;
}

function toError(value: unknown) {
  return value instanceof Error ? value : new Error(String(value));
}
