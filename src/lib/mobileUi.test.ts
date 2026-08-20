import { describe, expect, it } from "vitest";
import { isNearScrollBottom, shouldSubmitComposer } from "./mobileUi";

describe("mobile UI helpers", () => {
  it("does not submit while an IME composition is active", () => {
    expect(shouldSubmitComposer({ key: "Enter", shiftKey: false, isComposing: true })).toBe(false);
    expect(shouldSubmitComposer({ key: "Enter", shiftKey: false, keyCode: 229 })).toBe(false);
    expect(shouldSubmitComposer({ key: "Enter", shiftKey: false, isComposing: false })).toBe(true);
    expect(shouldSubmitComposer({ key: "Enter", shiftKey: true, isComposing: false })).toBe(false);
  });

  it("only auto-follows when the reader is near the bottom", () => {
    expect(isNearScrollBottom(620, 1000, 320)).toBe(true);
    expect(isNearScrollBottom(200, 1000, 320)).toBe(false);
  });
});
