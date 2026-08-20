import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import type { CodexModel, CodexThread, ServerRequest, WireMessage } from "./types";
import {
  GatewayAppServerClient,
  GatewaySession,
  type GatewayHost,
} from "./lib/gatewayClient";
import "./web.css";

type ThreadListResponse = { data: CodexThread[]; nextCursor?: string | null };
type ModelListResponse = { data: CodexModel[]; nextCursor?: string | null };

export default function WebApp() {
  const session = useMemo(() => new GatewaySession(), []);
  const clientRef = useRef<GatewayAppServerClient | null>(null);
  const activeThreadRef = useRef<CodexThread | null>(null);
  const [authenticated, setAuthenticated] = useState(false);
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [hosts, setHosts] = useState<GatewayHost[]>([]);
  const [host, setHost] = useState<GatewayHost | null>(null);
  const [status, setStatus] = useState("disconnected");
  const [threads, setThreads] = useState<CodexThread[]>([]);
  const [activeThread, setActiveThread] = useState<CodexThread | null>(null);
  const [models, setModels] = useState<CodexModel[]>([]);
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [requests, setRequests] = useState<ServerRequest[]>([]);

  useEffect(() => {
    void session.restore()
      .then(() => setAuthenticated(true))
      .catch(() => setAuthenticated(false));
    return () => clientRef.current?.disconnect();
  }, [session]);

  useEffect(() => {
    if (!authenticated) return;
    void refreshHosts();
  }, [authenticated]);

  async function refreshHosts() {
    try {
      setHosts(await session.listHosts());
      setError("");
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function login(event: FormEvent) {
    event.preventDefault();
    const submitted = password;
    setPassword("");
    try {
      await session.login(submitted);
      setAuthenticated(true);
      setError("");
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function selectHost(selected: GatewayHost) {
    clientRef.current?.disconnect();
    setHost(selected);
    setThreads([]);
    setActiveThread(null);
    activeThreadRef.current = null;
    setRequests([]);
    const client = session.connection(selected, {
      onNotification: handleNotification,
      onServerRequest: (request) => setRequests((current) => [request, ...current]),
      onStatus: setStatus,
      onResyncRequired: () => void loadCatalog(client),
      onCapabilities: (lunaMax) => {
        setHost((current) => current ? { ...current, lunaMax } : current);
      },
    });
    clientRef.current = client;
    try {
      setStatus("connecting");
      await client.connect();
      await loadCatalog(client);
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function loadCatalog(client = clientRef.current) {
    if (!client) return;
    try {
      const [threadPages, modelPages] = await Promise.all([
        listAllPages<CodexThread>(client, "thread/list", { archived: false, limit: 100 }),
        listAllPages<CodexModel>(client, "model/list", { limit: 100 }),
      ]);
      setThreads(threadPages);
      setModels(modelPages);
      const first = modelPages.find((entry) => entry.isDefault) ?? modelPages[0];
      if (first) {
        setModel(first.model);
        setEffort(first.defaultReasoningEffort);
      }
      setError("");
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function openThread(threadId: string) {
    const client = clientRef.current;
    if (!client) return;
    try {
      const response = await client.request<{ thread: CodexThread }>("thread/resume", { threadId });
      setActiveThread(response.thread);
      activeThreadRef.current = response.thread;
      setRequests([]);
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  async function sendTurn(event: FormEvent) {
    event.preventDefault();
    const client = clientRef.current;
    const text = prompt.trim();
    if (!client || !activeThread || !text || busy) return;
    setPrompt("");
    setBusy(true);
    try {
      await client.request("turn/start", {
        threadId: activeThread.id,
        input: [{ type: "text", text, text_elements: [] }],
        ...(model ? { model } : {}),
        ...(effort ? { effort } : {}),
      });
    } catch (reason) {
      setPrompt((current) => current || text);
      setBusy(false);
      setError(toMessage(reason));
    }
  }

  function handleNotification(message: WireMessage) {
    const threadId = typeof message.params?.threadId === "string" ? message.params.threadId : null;
    if (message.method === "turn/started" && threadId === activeThreadRef.current?.id) setBusy(true);
    if (message.method === "turn/completed" && threadId === activeThreadRef.current?.id) {
      setBusy(false);
      void openThread(threadId);
    }
  }

  if (!authenticated) {
    return (
      <main className="web-login">
        <form className="web-card" onSubmit={login}>
          <p className="web-kicker">CODEX REMOTE</p>
          <h1>Gatewayへログイン</h1>
          <p>Codexの資格情報ではなく、このGatewayのパスワードを入力します。</p>
          <label>パスワード<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" /></label>
          <button type="submit" disabled={!password}>ログイン</button>
          {error && <p role="alert" className="web-error">{error}</p>}
        </form>
      </main>
    );
  }

  return (
    <main className="web-shell">
      <aside className="web-sidebar">
        <div className="web-brand"><span>Codex Remote</span><small>{status}</small></div>
        <section>
          <div className="web-section-title"><span>Hosts</span><button onClick={() => void refreshHosts()}>更新</button></div>
          {hosts.map((entry) => <button className={host?.id === entry.id ? "active" : ""} key={entry.id} onClick={() => void selectHost(entry)}>{entry.displayName}<small>{entry.lunaMax?.state === "available" ? "Luna Max" : "接続可能"}</small></button>)}
          {!hosts.length && <p className="web-muted">Host Agentを起動してください。</p>}
        </section>
        <section className="web-thread-list">
          <div className="web-section-title"><span>Tasks</span><span>{threads.length}</span></div>
          {threads.map((thread) => <button className={activeThread?.id === thread.id ? "active" : ""} key={thread.id} onClick={() => void openThread(thread.id)}>{thread.name || thread.preview || "名称未設定"}<small>{new Date(thread.updatedAt * 1000).toLocaleString()}</small></button>)}
        </section>
      </aside>
      <section className="web-workspace">
        <header><strong>{activeThread?.name || activeThread?.preview || host?.displayName || "Hostを選択"}</strong><span>{busy ? "実行中" : status}</span></header>
        {error && <p role="alert" className="web-error web-banner">{error}</p>}
        <div className="web-conversation">
          {!activeThread && <div className="web-empty"><h2>既存タスクを選択</h2><p>新規Workspace作成は安全なWorkspace ID APIの実装後に有効になります。</p></div>}
          {activeThread?.turns.flatMap((turn) => turn.items).map((item, index) => <article key={item.id || index}><span>{item.type}</span><pre>{item.text || item.command || item.aggregatedOutput || JSON.stringify(item, null, 2)}</pre></article>)}
          {requests.map((request) => <article className="web-request" key={`${request.method}:${request.id}`}><strong>操作待ち: {request.method}</strong><pre>{JSON.stringify(request.params, null, 2)}</pre><p>安全な型別回答UIは移植中です。この画面から自動承認しません。</p></article>)}
        </div>
        <form className="web-composer" onSubmit={sendTurn}>
          <div><select value={model} onChange={(event) => { setModel(event.target.value); const selected = models.find((entry) => entry.model === event.target.value); setEffort(selected?.defaultReasoningEffort || ""); }}>{models.map((entry) => <option key={entry.model} value={entry.model}>{entry.displayName || entry.model}</option>)}</select><input value={effort} onChange={(event) => setEffort(event.target.value)} aria-label="推論レベル" /></div>
          <textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder="Codexへ指示" disabled={!activeThread} />
          <button type="submit" disabled={!activeThread || !prompt.trim() || busy}>送信</button>
        </form>
      </section>
    </main>
  );
}

async function listAllPages<T>(
  client: GatewayAppServerClient,
  method: "thread/list" | "model/list",
  baseParams: Record<string, unknown>,
) {
  const data: T[] = [];
  let cursor: string | null | undefined;
  const seen = new Set<string>();
  for (let page = 0; page < 100; page += 1) {
    const response = await client.request<ThreadListResponse | ModelListResponse>(method, { ...baseParams, cursor });
    data.push(...response.data as T[]);
    cursor = response.nextCursor;
    if (!cursor) return data;
    if (seen.has(cursor)) throw new Error(`${method} のcursorが循環しました`);
    seen.add(cursor);
  }
  throw new Error(`${method} がページ上限を超えました`);
}

function toMessage(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}
