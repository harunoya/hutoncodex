import { describe, expect, it } from "vitest";
import type { CodexThread } from "./types";
import { groupThreads, mergeItemIntoThread, mergeTurnIntoThread, projectName, threadTitle } from "./WebApp";

function thread(overrides: Partial<CodexThread>): CodexThread {
  return {
    id: "thread-1",
    sessionId: "session-1",
    preview: "Preview",
    name: null,
    cwd: "C:\\work\\codexremote",
    modelProvider: "openai",
    createdAt: 1,
    updatedAt: 1,
    recencyAt: 1,
    status: { type: "idle" },
    turns: [],
    ...overrides,
  };
}

describe("Codex-style Web task navigation", () => {
  it("groups tasks by their App Server workspace and orders recent projects first", () => {
    const groups = groupThreads([
      thread({ id: "old", cwd: "C:\\work\\alpha", updatedAt: 10 }),
      thread({ id: "new", cwd: "C:\\work\\beta", updatedAt: 30 }),
      thread({ id: "middle", cwd: "C:\\work\\alpha", updatedAt: 20 }),
    ], "");

    expect(groups.map((group) => group.project)).toEqual(["beta", "alpha"]);
    expect(groups[1].threads.map((entry) => entry.id)).toEqual(["middle", "old"]);
  });

  it("filters only the fetched task title and workspace", () => {
    const groups = groupThreads([
      thread({ id: "one", name: "Relayを確認", cwd: "C:\\work\\remote" }),
      thread({ id: "two", name: "UIを調整", cwd: "C:\\work\\client" }),
    ], "client");

    expect(groups).toHaveLength(1);
    expect(groups[0].threads[0].id).toBe("two");
  });

  it("uses App Server names and safe fallbacks without inventing tasks", () => {
    expect(threadTitle(thread({ name: "  実装タスク  " }))).toBe("実装タスク");
    expect(threadTitle(thread({ name: null, preview: "  preview  " }))).toBe("preview");
    expect(threadTitle(thread({ name: null, preview: "" }))).toBe("名称未設定");
    expect(projectName("/home/user/project")).toBe("project");
  });

  it("keeps streamed items when a partial completed turn arrives", () => {
    const current = thread({
      turns: [{
        id: "turn-1",
        items: [{ type: "agentMessage", id: "message-1", text: "表示済み" }],
        status: "inProgress",
        error: null,
        startedAt: 1,
        completedAt: null,
        durationMs: null,
      }],
    });
    const merged = mergeTurnIntoThread(current, {
      id: "turn-1",
      items: [],
      status: "completed",
      error: null,
      startedAt: 1,
      completedAt: 2,
      durationMs: 1,
    });

    expect(merged.turns[0].items[0].text).toBe("表示済み");
    expect(merged.turns[0].status).toBe("completed");
  });

  it("updates an App Server item without duplicating it", () => {
    const current = thread({
      turns: [{
        id: "turn-1",
        items: [{ type: "commandExecution", id: "command-1", status: "inProgress" }],
        status: "inProgress",
        error: null,
        startedAt: 1,
        completedAt: null,
        durationMs: null,
      }],
    });

    const merged = mergeItemIntoThread(current, "turn-1", { type: "commandExecution", id: "command-1", status: "completed" });
    expect(merged.turns[0].items).toHaveLength(1);
    expect(merged.turns[0].items[0].status).toBe("completed");
  });
});
