// @vitest-environment jsdom

import type { ComponentType } from "react";
import { act } from "react";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const scanner = vi.hoisted(() => ({
  pending: null as Promise<{ stop: () => void }> | null,
  resolve: undefined as ((controls: { stop: () => void }) => void) | undefined,
  stopCount: 0,
}));

const pairing = vi.hoisted(() => ({
  codes: [] as string[],
}));

vi.mock("@zxing/browser", () => ({
  BrowserQRCodeReader: class {
    decodeFromVideoDevice() {
      scanner.pending = new Promise((resolve) => {
        scanner.resolve = resolve;
      });
      return scanner.pending;
    }

    async decodeFromImageUrl() {
      throw new Error("invalid image");
    }
  },
}));

vi.mock("./lib/appServer", () => ({
  AppServerClient: class {
    async mount() {}
    async preparePair() {}
    async connectPair(code: string) {
      pairing.codes.push(code);
      throw new Error("pair failed");
    }
    async connect() {
      return {
        userAgent: "test",
        codexHome: "~/.codex",
        platformFamily: "windows",
        platformOs: "Windows",
      };
    }
    getConnectionId() {
      return 999;
    }
    recordPhase() {}
    dispose() {}
    async disconnect() {}
    async request() {
      return {};
    }
    async respond() {}
    async respondError() {}
  },
}));

let App: ComponentType;

beforeAll(async () => {
  window.history.replaceState({}, "", "/?preview=active");
  Object.defineProperty(Element.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
  App = (await import("./App")).default;
});

afterAll(() => {
  window.history.replaceState({}, "", "/");
});

beforeEach(() => {
  scanner.pending = null;
  scanner.resolve = undefined;
  scanner.stopCount = 0;
  pairing.codes.length = 0;
});

afterEach(() => cleanup());

function setViewport(width: number, height: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  Object.defineProperty(window, "innerHeight", { configurable: true, value: height });
  fireEvent(window, new Event("resize"));
}

function openConnectionManager() {
  fireEvent.click(screen.getByTitle("接続先を管理"));
  return screen.getByRole("dialog", { name: "接続先" });
}

function selectConnection(label: string) {
  const row = screen.getByText(label).closest("button");
  if (!row) throw new Error(`${label} connection button was not found`);
  fireEvent.click(row);
}

function openConnectDialog() {
  openConnectionManager();
  fireEvent.click(screen.getByRole("button", { name: "接続を追加" }));
  return screen.getByRole("dialog", { name: "接続を追加" });
}

describe("connection-scoped UI state", () => {
  it("keeps independent drafts while switching A to B to A", async () => {
    render(<App />);
    const composer = screen.getByPlaceholderText("何でもどうぞ") as HTMLTextAreaElement;

    fireEvent.change(composer, { target: { value: "draft-a" } });
    openConnectionManager();
    selectConnection("devbox-tokyo");
    await waitFor(() => expect(composer.value).toBe(""));

    fireEvent.change(composer, { target: { value: "draft-b" } });
    openConnectionManager();
    selectConnection("remote-workspace");
    await waitFor(() => expect(composer.value).toBe("draft-a"));

    openConnectionManager();
    selectConnection("devbox-tokyo");
    await waitFor(() => expect(composer.value).toBe("draft-b"));
  });

  it("keeps model selection scoped to each connection", async () => {
    render(<App />);
    const modelTrigger = screen.getByRole("button", { name: "モデルを選択" });
    expect(modelTrigger.textContent).toContain("5.6 Sol");

    fireEvent.click(modelTrigger);
    fireEvent.click(screen.getByRole("menuitemradio", { name: /5.6 Terra/ }));
    expect(modelTrigger.textContent).toContain("5.6 Terra");

    openConnectionManager();
    selectConnection("devbox-tokyo");
    await waitFor(() => expect(modelTrigger.textContent).toContain("5.6 Sol"));

    openConnectionManager();
    selectConnection("remote-workspace");
    await waitFor(() => expect(modelTrigger.textContent).toContain("5.6 Terra"));
  });

  it("closes the connection manager with Escape", async () => {
    render(<App />);
    openConnectionManager();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "接続先" })).toBeNull();
    });
  });

  it("closes the connect dialog with Escape when it is idle", async () => {
    render(<App />);
    openConnectDialog();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "接続を追加" })).toBeNull();
    });
  });

  it("clears a Pair-code error when the connection method changes", async () => {
    render(<App />);
    openConnectDialog();
    fireEvent.click(screen.getByRole("button", { name: "Codex認証を準備" }));
    await screen.findByText(/準備完了/);
    fireEvent.change(screen.getByPlaceholderText("接続先に表示されたコード"), {
      target: { value: "InVaLiD-code" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Pairして接続" }));
    await screen.findByText("pair failed");
    expect(pairing.codes).toEqual(["InVaLiD-code"]);

    fireEvent.click(screen.getByRole("button", { name: "QR Pair" }));
    expect(screen.queryByText("pair failed")).toBeNull();
  });

  it("locks dismissal and method changes while camera permission is pending", async () => {
    render(<App />);
    const dialog = openConnectDialog();
    fireEvent.click(screen.getByRole("button", { name: "Codex認証を準備" }));
    await screen.findByText(/準備完了/);
    fireEvent.click(screen.getByRole("button", { name: "QR Pair" }));
    fireEvent.click(screen.getByRole("button", { name: "カメラで読取" }));

    const starting = await screen.findByRole("button", { name: "カメラを起動中" });
    expect((starting as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Pairコード" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "上級者向け" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.mouseDown(dialog.parentElement as HTMLElement);
    fireEvent.click(screen.getByRole("button", { name: "キャンセル" }));
    expect(screen.getByRole("dialog", { name: "接続を追加" })).not.toBeNull();

    await act(async () => {
      scanner.resolve?.({ stop: () => { scanner.stopCount += 1; } });
      await scanner.pending;
    });
    await screen.findByRole("button", { name: "カメラを停止" });

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "接続を追加" })).toBeNull();
    });
    expect(scanner.stopCount).toBe(1);
  });
});

describe("mobile interaction", () => {
  afterEach(() => setViewport(1024, 768));

  it("closes the task drawer with outside tap and Escape while blocking the page", async () => {
    setViewport(360, 800);
    render(<App />);
    const taskNavigation = screen.getByRole("button", { name: "タスク" });
    taskNavigation.focus();
    fireEvent.click(taskNavigation);

    const drawer = await screen.findByRole("dialog", { name: "タスク一覧" });
    expect(document.querySelector("main")?.hasAttribute("inert")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "サイドバーを閉じる" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "タスク一覧" })).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(taskNavigation));

    fireEvent.click(taskNavigation);
    await screen.findByRole("dialog", { name: "タスク一覧" });
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "タスク一覧" })).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(taskNavigation));
    expect(drawer).not.toBeNull();
  });

  it("keeps only one mobile layer open and dismisses it with Escape", async () => {
    setViewport(360, 800);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "タスク" }));
    const drawer = await screen.findByRole("dialog", { name: "タスク一覧" });
    fireEvent.click(within(drawer).getByTitle("使用状況"));
    await screen.findByRole("dialog", { name: "設定" });
    expect(screen.queryByRole("dialog", { name: "タスク一覧" })).toBeNull();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "設定" })).toBeNull());
  });

  it("does not send the composer draft while IME composition is active", () => {
    setViewport(360, 420);
    render(<App />);
    const composer = screen.getByLabelText("Codexへのメッセージ") as HTMLTextAreaElement;
    fireEvent.change(composer, { target: { value: "変換中" } });
    fireEvent.keyDown(composer, { key: "Enter", isComposing: true, keyCode: 229 });
    expect(composer.value).toBe("変換中");
  });
});
