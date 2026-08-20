import type { CodexThread } from "../types";

type RequestClient = {
  request<T>(method: string, params?: unknown, timeoutMs?: number): Promise<T>;
};

type ThreadPage = {
  data: CodexThread[];
  nextCursor: string | null;
};

export async function listAllThreads(client: RequestClient, pageSize = 100) {
  const threads: CodexThread[] = [];
  const seenIds = new Set<string>();
  const seenCursors = new Set<string>();
  let cursor: string | null = null;

  do {
    const page: ThreadPage = await client.request("thread/list", {
      cursor,
      limit: pageSize,
      sortKey: "updated_at",
      sortDirection: "desc",
      archived: false,
    });
    for (const thread of page.data) {
      if (!seenIds.has(thread.id)) {
        seenIds.add(thread.id);
        threads.push(thread);
      }
    }
    cursor = page.nextCursor;
    if (cursor) {
      if (seenCursors.has(cursor)) throw new Error("thread/list が同じカーソルを繰り返しました");
      seenCursors.add(cursor);
    }
  } while (cursor);

  return threads;
}
