import { describe, expect, it } from "vitest";
import type { CodexThread } from "../types";
import { listAllThreads } from "./catalogs";

function thread(id: string) {
  return { id } as CodexThread;
}

describe("listAllThreads", () => {
  it("loads every page after the first 100 threads", async () => {
    const calls: unknown[] = [];
    const first = Array.from({ length: 100 }, (_, index) => thread(String(index)));
    const client = {
      async request<T>(_method: string, params?: unknown): Promise<T> {
        calls.push(params);
        const cursor = (params as { cursor: string | null }).cursor;
        return (cursor
          ? { data: [thread("100")], nextCursor: null }
          : { data: first, nextCursor: "page-2" }) as T;
      },
    };

    const result = await listAllThreads(client);
    expect(result).toHaveLength(101);
    expect(calls).toHaveLength(2);
  });
});
