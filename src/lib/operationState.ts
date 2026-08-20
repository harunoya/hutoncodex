import type {
  JsonId,
  ManagedConnection,
  ServerRequest,
  ThreadItem,
  Turn,
} from "../types";

export type OperationToken = {
  connectionId: string;
  subjectId: string | null;
  generation: number;
};

export class LatestOperationGate {
  private generation = 0;

  begin(connectionId: string, subjectId: string | null): OperationToken {
    return { connectionId, subjectId, generation: ++this.generation };
  }

  invalidate() {
    this.generation += 1;
  }

  isCurrent(
    token: OperationToken,
    activeConnectionId: string | null,
    activeSubjectId: string | null,
  ) {
    return token.generation === this.generation
      && token.connectionId === activeConnectionId
      && token.subjectId === activeSubjectId;
  }
}

export function removePendingRequest(requests: ServerRequest[], requestId: JsonId) {
  return requests.filter((request) => request.id !== requestId);
}

export function removeConnection(
  connections: ManagedConnection[],
  connectionId: string,
) {
  return connections.filter((connection) => connection.id !== connectionId);
}

export function requestCardKey(connectionId: string | null, requestId: JsonId) {
  return `${connectionId ?? "none"}:${String(requestId)}`;
}

export function shouldApplyThreadBusy(
  activeConnectionId: string | null,
  ownerConnectionId: string,
  activeThreadId: string | null,
  eventThreadId: string | null,
) {
  return activeConnectionId === ownerConnectionId
    && activeThreadId !== null
    && activeThreadId === eventThreadId;
}

export function turnInterruptParams(threadId: string, turnId: string) {
  return { threadId, turnId };
}

export function restoreDraftAfterFailure(currentDraft: string, sentPrompt: string) {
  return currentDraft || sentPrompt;
}

export function mergeCompletedTurn(turns: Turn[] = [], completed: Turn) {
  const index = turns.findIndex((turn) => turn.id === completed.id);
  if (index < 0) return [...turns, completed];

  const copy = [...turns];
  const current = copy[index];
  copy[index] = {
    ...current,
    ...completed,
    items: mergeTurnItems(current.items, completed.items),
  };
  return copy;
}

export function uniqueConnectionLabel(base: string, existing: ManagedConnection[]) {
  const labels = new Set(existing.map((connection) => connection.label));
  if (!labels.has(base)) return base;
  let suffix = 2;
  while (labels.has(`${base} (${suffix})`)) suffix += 1;
  return `${base} (${suffix})`;
}

function mergeTurnItems(current: ThreadItem[] = [], completed: ThreadItem[] = []) {
  if (!completed.length) return current;

  const items = current.map((item) => ({ ...item }));
  for (let incomingIndex = 0; incomingIndex < completed.length; incomingIndex += 1) {
    const incoming = completed[incomingIndex];
    const existingIndex = incoming.id
      ? items.findIndex((item) => item.id === incoming.id)
      : items.findIndex((item, index) => !item.id && index === incomingIndex && item.type === incoming.type);
    if (existingIndex < 0) items.push(incoming);
    else items[existingIndex] = { ...items[existingIndex], ...incoming };
  }
  return items;
}
