import { beforeEach, describe, expect, it, vi } from "vitest";

type Listener = (event: { payload: any }) => void;
type Deferred = { promise: Promise<{ connectionId: number }>; resolve: (value: { connectionId: number }) => void };

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  listeners: new Map<string, Set<Listener>>(),
  unlisten: vi.fn(),
  deferred: new Map<string, Deferred>(),
  blockedInitialize: new Set<number>(),
  sentMethods: [] as string[],
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauri.listen }));

import { AppServerClient } from "./appServer";

function deferred(): Deferred {
  let resolve!: (value: { connectionId: number }) => void;
  const promise = new Promise<{ connectionId: number }>((done) => { resolve = done; });
  return { promise, resolve };
}

function emit(name: string, payload: unknown) {
  for (const listener of tauri.listeners.get(name) ?? []) listener({ payload });
}

function makeClient() {
  return new AppServerClient({
    onNotification: vi.fn(),
    onServerRequest: vi.fn(),
    onStatus: vi.fn(),
  });
}

beforeEach(() => {
  tauri.invoke.mockReset();
  tauri.listen.mockReset();
  tauri.unlisten.mockReset();
  tauri.listeners.clear();
  tauri.deferred.clear();
  tauri.sentMethods.length = 0;
  tauri.blockedInitialize.clear();
  tauri.listen.mockImplementation(async (name: string, listener: Listener) => {
    const listeners = tauri.listeners.get(name) ?? new Set<Listener>();
    listeners.add(listener);
    tauri.listeners.set(name, listeners);
    return () => {
      listeners.delete(listener);
      tauri.unlisten(name);
    };
  });
  tauri.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "connect_app_server") {
      const pending = deferred();
      tauri.deferred.set(args.url, pending);
      return pending.promise;
    }
    if (command === "send_app_server_message") {
      const method = args.message.method as string | undefined;
      if (method) tauri.sentMethods.push(method);
      if (method === "initialize") {
        if (tauri.blockedInitialize.has(args.connectionId)) return undefined;
        queueMicrotask(() => emit("app-server-message", {
          connectionId: args.connectionId,
          message: {
            id: args.message.id,
            result: {
              userAgent: "test",
              codexHome: "~/.codex",
              platformFamily: "windows",
              platformOs: "Windows",
            },
          },
        }));
      }
      return undefined;
    }
    return undefined;
  });
});

describe("AppServerClient connection lifecycle", () => {
  it("prepares Pair auth through a cancellable Tauri attempt", async () => {
    const client = makeClient();
    await client.mount();
    await client.preparePair();
    expect(tauri.invoke.mock.calls.some(([command]) => command === "prepare_pair_connection"))
      .toBe(true);
    client.dispose();
  });

  it("registers one global listener set for multiple clients", async () => {
    const first = makeClient();
    const second = makeClient();
    await Promise.all([first.mount(), second.mount()]);
    expect(tauri.listen).toHaveBeenCalledTimes(4);
    first.dispose();
    second.dispose();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(tauri.unlisten).toHaveBeenCalledTimes(4);
  });

  it("keeps B when A finishes after B and disconnects stale A", async () => {
    const client = makeClient();
    await client.mount();
    const attemptA = client.connect("ws://a");
    const attemptB = client.connect("ws://b");
    tauri.deferred.get("ws://b")!.resolve({ connectionId: 22 });
    const resultB = await attemptB;
    expect(resultB.userAgent).toBe("test");
    tauri.deferred.get("ws://a")!.resolve({ connectionId: 11 });
    await expect(attemptA).rejects.toThrow("古い接続試行");
    expect(tauri.invoke).toHaveBeenCalledWith("disconnect_app_server", { connectionId: 11 });
    expect(client.getConnectionId()).toBe(22);
    client.dispose();
  });

  it("does not let a cancelled A initializer clear connected B", async () => {
    const client = makeClient();
    await client.mount();
    tauri.blockedInitialize.add(11);
    const attemptA = client.connect("ws://initializing-a");
    tauri.deferred.get("ws://initializing-a")!.resolve({ connectionId: 11 });
    await new Promise((resolve) => setTimeout(resolve, 0));
    const rejectedA = expect(attemptA).rejects.toThrow("新しい接続試行");
    const attemptB = client.connect("ws://replacement-b");
    tauri.deferred.get("ws://replacement-b")!.resolve({ connectionId: 22 });
    await attemptB;
    await rejectedA;
    expect(client.getConnectionId()).toBe(22);
    client.dispose();
  });

  it("cancels an in-flight attempt and rejects a late connection after dispose", async () => {
    const client = makeClient();
    await client.mount();
    const attempt = client.connect("ws://late");
    client.dispose();
    expect(tauri.invoke.mock.calls.some(([command]) => command === "cancel_connection_attempt"))
      .toBe(true);
    tauri.deferred.get("ws://late")!.resolve({ connectionId: 33 });
    await expect(attempt).rejects.toThrow("古い接続試行");
    expect(tauri.invoke).toHaveBeenCalledWith("disconnect_app_server", { connectionId: 33 });
  });

  it("cancels an in-flight attempt through the public cancellation API", async () => {
    const client = makeClient();
    await client.mount();
    const attempt = client.connect("ws://cancel-me");

    await client.cancelConnectionAttempt();

    expect(tauri.invoke.mock.calls.some(([command]) => command === "cancel_connection_attempt"))
      .toBe(true);
    tauri.deferred.get("ws://cancel-me")!.resolve({ connectionId: 34 });
    await expect(attempt).rejects.toThrow("古い接続試行");
    expect(tauri.invoke).toHaveBeenCalledWith("disconnect_app_server", { connectionId: 34 });
    client.dispose();
  });

  it("sends initialized only after initialize completes", async () => {
    const client = makeClient();
    await client.mount();
    const attempt = client.connect("ws://ordered");
    tauri.deferred.get("ws://ordered")!.resolve({ connectionId: 44 });
    await attempt;
    expect(tauri.sentMethods.slice(0, 2)).toEqual(["initialize", "initialized"]);
    client.dispose();
  });
});
