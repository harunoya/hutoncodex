import { describe, expect, it } from "vitest";
import { parseAgentEvent } from "./gatewayClient";

describe("parseAgentEvent", () => {
  it("accepts an owned app-server envelope", () => {
    expect(parseAgentEvent(JSON.stringify({
      type: "appServerMessage",
      envelope: {
        hostId: "host-a",
        connectionGeneration: 3,
        sequence: 7,
        message: { method: "turn/completed", params: { threadId: "thread-a" } },
      },
    })))?.toMatchObject({ type: "appServerMessage" });
  });

  it("rejects malformed envelopes", () => {
    expect(parseAgentEvent('{"type":"appServerMessage","envelope":{"sequence":1}}')).toBeNull();
    expect(parseAgentEvent("not json")).toBeNull();
  });
});
