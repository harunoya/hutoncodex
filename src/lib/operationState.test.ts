import { describe, expect, it } from "vitest";
import type { ManagedConnection, ServerRequest } from "../types";
import {
  LatestOperationGate,
  mergeCompletedTurn,
  removeConnection,
  removePendingRequest,
  requestCardKey,
  restoreDraftAfterFailure,
  shouldApplyThreadBusy,
  turnInterruptParams,
  uniqueConnectionLabel,
} from "./operationState";
import type { Turn } from "../types";

describe("connection-scoped operation state", () => {
  it("accepts B and rejects A when A then B complete out of order", () => {
    const gate = new LatestOperationGate();
    const a = gate.begin("connection", "thread-a");
    const b = gate.begin("connection", "thread-b");

    expect(gate.isCurrent(b, "connection", "thread-b")).toBe(true);
    expect(gate.isCurrent(a, "connection", "thread-b")).toBe(false);
  });

  it("rejects an operation after switching connections", () => {
    const gate = new LatestOperationGate();
    const operation = gate.begin("a", "thread-a");
    expect(gate.isCurrent(operation, "b", "thread-a")).toBe(false);
  });

  it("removes concurrent disconnect targets from the latest list", () => {
    const base = ["a", "b", "c"].map((id) => ({ id })) as ManagedConnection[];
    const afterA = removeConnection(base, "a");
    const afterB = removeConnection(afterA, "b");
    expect(afterB.map((connection) => connection.id)).toEqual(["c"]);
  });

  it("keeps a request that arrives while another request is resolved", () => {
    const first = { id: 1, method: "first", params: {} } as ServerRequest;
    const second = { id: 2, method: "second", params: {} } as ServerRequest;
    expect(removePendingRequest([first, second], first.id)).toEqual([second]);
  });

  it("resolves a request only in connection A after the UI switches to B", () => {
    const requestA = { id: 1, method: "approval-a", params: {} } as ServerRequest;
    const requestB = { id: 1, method: "approval-b", params: {} } as ServerRequest;
    const pending = new Map<string, ServerRequest[]>([
      ["a", [requestA]],
      ["b", [requestB]],
    ]);
    const ownerConnectionId = "a";
    const activeConnectionId = "b";

    pending.set(
      ownerConnectionId,
      removePendingRequest(pending.get(ownerConnectionId) ?? [], requestA.id),
    );

    expect(activeConnectionId).toBe("b");
    expect(pending.get("a")).toEqual([]);
    expect(pending.get("b")).toEqual([requestB]);
  });

  it("separates equal request ids from different connections", () => {
    expect(requestCardKey("a", 1)).not.toBe(requestCardKey("b", 1));
  });

  it("ignores busy notifications for a background thread", () => {
    expect(shouldApplyThreadBusy("a", "a", "thread-a", "thread-b")).toBe(false);
    expect(shouldApplyThreadBusy("a", "a", "thread-a", "thread-a")).toBe(true);
  });

  it("includes the required turn id in an interrupt request", () => {
    expect(turnInterruptParams("thread-a", "turn-a")).toEqual({
      threadId: "thread-a",
      turnId: "turn-a",
    });
  });

  it("preserves streamed items when a completed turn omits them", () => {
    const streamed = {
      id: "turn-a",
      status: "inProgress",
      items: [
        { id: "user-a", type: "userMessage", text: "question" },
        { id: "agent-a", type: "agentMessage", text: "answer" },
      ],
      error: null,
      startedAt: 1,
      completedAt: null,
      durationMs: null,
    } satisfies Turn;
    const completed = {
      ...streamed,
      status: "completed",
      items: [],
      completedAt: 2,
      durationMs: 1,
    } satisfies Turn;

    expect(mergeCompletedTurn([streamed], completed)).toEqual([{
      ...completed,
      items: streamed.items,
    }]);
  });

  it("does not overwrite a newer draft when an older send fails", () => {
    expect(restoreDraftAfterFailure("new draft", "failed prompt")).toBe("new draft");
    expect(restoreDraftAfterFailure("", "failed prompt")).toBe("failed prompt");
  });

  it("merges completed item fields without dropping other streamed items", () => {
    const streamed = {
      id: "turn-a",
      status: "inProgress",
      items: [
        { id: "user-a", type: "userMessage", text: "question" },
        { id: "command-a", type: "commandExecution", aggregatedOutput: "partial" },
      ],
      error: null,
      startedAt: 1,
      completedAt: null,
      durationMs: null,
    } satisfies Turn;
    const completed = {
      ...streamed,
      status: "completed",
      items: [{
        id: "command-a",
        type: "commandExecution",
        aggregatedOutput: "complete",
        exitCode: 0,
      }],
      completedAt: 2,
      durationMs: 1,
    } satisfies Turn;

    expect(mergeCompletedTurn([streamed], completed)[0].items).toEqual([
      streamed.items[0],
      completed.items[0],
    ]);
  });

  it("assigns stable unique labels to duplicate endpoints", () => {
    const existing = [
      { label: "127.0.0.1:4500" },
      { label: "127.0.0.1:4500 (2)" },
    ] as ManagedConnection[];
    expect(uniqueConnectionLabel("127.0.0.1:4500", existing)).toBe("127.0.0.1:4500 (3)");
    expect(uniqueConnectionLabel("other:4500", existing)).toBe("other:4500");
  });
});
