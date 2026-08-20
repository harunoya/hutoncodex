import { describe, expect, it } from "vitest";
import { deriveDiscordPresence } from "./discordPresence";

describe("Discord presence mapping", () => {
  it("maps each connection method to a distinct connecting state", () => {
    const base = {
      connectionAdding: true,
      connectionError: false,
      connected: false,
      busy: false,
      hasSelectedTask: false,
    };
    expect(deriveDiscordPresence({ ...base, connectionMode: "manual" }).kind).toBe("connectingPair");
    expect(deriveDiscordPresence({ ...base, connectionMode: "qr" }).kind).toBe("connectingQr");
    expect(deriveDiscordPresence({ ...base, connectionMode: "advanced" }).kind).toBe("connectingAppServer");
  });

  it("prioritizes input and approval waiting over working", () => {
    const base = {
      connectionAdding: false,
      connectionMode: "advanced" as const,
      connectionError: false,
      connected: true,
      busy: true,
      hasSelectedTask: true,
      taskName: "task",
    };
    expect(deriveDiscordPresence({ ...base, pendingMethod: "item/tool/requestUserInput" }).kind)
      .toBe("waitingInput");
    expect(deriveDiscordPresence({ ...base, pendingMethod: "item/commandExecution/requestApproval" }).kind)
      .toBe("waitingApproval");
  });

  it("passes a task name only while working", () => {
    const working = deriveDiscordPresence({
      connectionAdding: false,
      connectionMode: "advanced",
      connectionError: false,
      connected: true,
      busy: true,
      hasSelectedTask: true,
      taskName: "visible task",
    });
    const idle = deriveDiscordPresence({
      connectionAdding: false,
      connectionMode: "advanced",
      connectionError: false,
      connected: true,
      busy: false,
      hasSelectedTask: true,
      taskName: "visible task",
    });
    expect(working.taskName).toBe("visible task");
    expect(idle.taskName).toBeNull();
  });
});
