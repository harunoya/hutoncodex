import {
  FormEvent,
  KeyboardEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  ChevronDown,
  ChevronRight,
  CircleAlert,
  Folder,
  LoaderCircle,
  Monitor,
  PanelLeft,
  RefreshCw,
  Search,
  Send,
  Square,
  TerminalSquare,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type {
  CodexModel,
  CodexThread,
  ServerRequest,
  ThreadItem,
  Turn,
  WireMessage,
} from "./types";
import {
  GatewayAppServerClient,
  GatewaySession,
  type GatewayHost,
} from "./lib/gatewayClient";
import "./web.css";

type ThreadListResponse = { data: CodexThread[]; nextCursor?: string | null };
type ModelListResponse = { data: CodexModel[]; nextCursor?: string | null };
type PendingApproval = { request: ServerRequest; client: GatewayAppServerClient; hostId: string };

export default function WebApp() {
  const session = useMemo(() => new GatewaySession(), []);
  const clientRef = useRef<GatewayAppServerClient | null>(null);
  const activeThreadRef = useRef<CodexThread | null>(null);
  const activeTurnIdRef = useRef<string | null>(null);
  const openThreadGenerationRef = useRef(0);
  const conversationRef = useRef<HTMLDivElement | null>(null);
  const followOutputRef = useRef(true);
  const [authenticated, setAuthenticated] = useState(false);
  const [restoring, setRestoring] = useState(true);
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
  const [activeTurnId, setActiveTurnId] = useState<string | null>(null);
  const [streamText, setStreamText] = useState("");
  const [approvals, setApprovals] = useState<PendingApproval[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [openingThreadId, setOpeningThreadId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [hostMenuOpen, setHostMenuOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);

  useEffect(() => {
    void session.restore()
      .then(() => setAuthenticated(true))
      .catch(() => setAuthenticated(false))
      .finally(() => setRestoring(false));
    return () => clientRef.current?.disconnect();
  }, [session]);

  useEffect(() => {
    if (authenticated) void refreshHosts();
  }, [authenticated]);

  useEffect(() => {
    if (!followOutputRef.current) return;
    const element = conversationRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [activeThread, streamText, approvals]);

  const groupedThreads = useMemo(() => groupThreads(threads, search), [threads, search]);
  const selectedModel = models.find((entry) => entry.model === model);
  const supportedEfforts = selectedModel?.supportedReasoningEfforts ?? [];

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

  async function logout() {
    clientRef.current?.disconnect();
    try {
      await session.logout();
    } finally {
      setAuthenticated(false);
      setHost(null);
      setThreads([]);
      setActiveThread(null);
      setHostMenuOpen(false);
    }
  }

  async function selectHost(selected: GatewayHost) {
    clientRef.current?.disconnect();
    openThreadGenerationRef.current += 1;
    activeTurnIdRef.current = null;
    setActiveTurnId(null);
    setBusy(false);
    setHost(selected);
    setHostMenuOpen(false);
    setSidebarOpen(false);
    setThreads([]);
    setActiveThread(null);
    activeThreadRef.current = null;
    setApprovals([]);
    setModels([]);
    setStreamText("");
    setError("");
    let ownerClient: GatewayAppServerClient;
    ownerClient = session.connection(selected, {
      onNotification: handleNotification,
      onServerRequest: (request) => void handleServerRequest(ownerClient, selected.id, request),
      onStatus: (nextStatus, detail) => {
        if (clientRef.current !== ownerClient) return;
        setStatus(nextStatus);
        if (detail) setError(detail);
      },
      onResyncRequired: () => void loadCatalog(ownerClient),
      onCapabilities: (lunaMax) => {
        if (clientRef.current !== ownerClient) return;
        setHost((current) => current?.id === selected.id ? { ...current, lunaMax } : current);
      },
    });
    clientRef.current = ownerClient;
    try {
      setStatus("connecting");
      await ownerClient.connect();
      await loadCatalog(ownerClient);
    } catch (reason) {
      if (clientRef.current === ownerClient) setError(toMessage(reason));
    }
  }

  async function loadCatalog(client = clientRef.current) {
    if (!client) return;
    setCatalogLoading(true);
    try {
      const [threadPages, modelPages] = await Promise.all([
        listAllPages<CodexThread>(client, "thread/list", { archived: false, limit: 100 }),
        listAllPages<CodexModel>(client, "model/list", { limit: 100 }),
      ]);
      if (clientRef.current !== client) return;
      setThreads(threadPages);
      setModels(modelPages.filter((entry) => !entry.hidden));
      const first = modelPages.find((entry) => entry.isDefault) ?? modelPages[0];
      if (first) {
        setModel(first.model);
        setEffort(first.defaultReasoningEffort);
      }
      setError("");
    } catch (reason) {
      if (clientRef.current === client) setError(toMessage(reason));
    } finally {
      if (clientRef.current === client) setCatalogLoading(false);
    }
  }

  async function openThread(threadId: string) {
    const client = clientRef.current;
    if (!client) return;
    const generation = ++openThreadGenerationRef.current;
    setOpeningThreadId(threadId);
    setSidebarOpen(false);
    setStreamText("");
    followOutputRef.current = true;
    try {
      const response = await client.request<{ thread: CodexThread }>("thread/resume", { threadId });
      if (clientRef.current !== client || openThreadGenerationRef.current !== generation) return;
      setActiveThread(response.thread);
      activeThreadRef.current = response.thread;
      setApprovals((current) => current.filter((entry) => entry.hostId !== host?.id));
      setError("");
    } catch (reason) {
      if (clientRef.current === client) setError(toMessage(reason));
    } finally {
      if (clientRef.current === client) setOpeningThreadId((current) => current === threadId ? null : current);
    }
  }

  async function sendTurn(event: FormEvent) {
    event.preventDefault();
    const client = clientRef.current;
    const text = prompt.trim();
    if (!client || !activeThreadRef.current || !text || busy) return;
    const threadId = activeThreadRef.current.id;
    setPrompt("");
    setBusy(true);
    setStreamText("");
    followOutputRef.current = true;
    try {
      await client.request("turn/start", {
        threadId,
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

  async function interruptTurn() {
    const client = clientRef.current;
    const threadId = activeThreadRef.current?.id;
    const turnId = activeTurnIdRef.current;
    if (!client || !threadId || !turnId) return;
    try {
      await client.request("turn/interrupt", { threadId, turnId });
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  function handleComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  }

  function handleNotification(message: WireMessage) {
    const threadId = typeof message.params?.threadId === "string" ? message.params.threadId : null;
    if ((message.method === "item/started" || message.method === "item/completed") && threadId === activeThreadRef.current?.id) {
      const turnId = typeof message.params?.turnId === "string" ? message.params.turnId : null;
      const item = isRecord(message.params?.item) ? message.params.item as ThreadItem : null;
      if (turnId && item) updateVisibleThread((thread) => mergeItemIntoThread(thread, turnId, item));
      return;
    }
    if (message.method === "item/agentMessage/delta" && threadId === activeThreadRef.current?.id) {
      const delta = typeof message.params?.delta === "string" ? message.params.delta : "";
      setStreamText((current) => current + delta);
      return;
    }
    if (message.method === "turn/started" && threadId === activeThreadRef.current?.id) {
      const turn = isRecord(message.params?.turn) ? message.params.turn : null;
      const turnId = turn && typeof turn.id === "string" ? turn.id : null;
      if (turnId && turn && Array.isArray(turn.items)) updateVisibleThread((thread) => mergeTurnIntoThread(thread, turn as unknown as Turn));
      activeTurnIdRef.current = turnId;
      setActiveTurnId(turnId);
      setBusy(true);
      return;
    }
    if (message.method === "turn/completed" && threadId === activeThreadRef.current?.id) {
      const completedTurn = isRecord(message.params?.turn) && typeof message.params.turn.id === "string" && Array.isArray(message.params.turn.items)
        ? message.params.turn as unknown as Turn
        : null;
      if (completedTurn) updateVisibleThread((thread) => mergeTurnIntoThread(thread, completedTurn));
      setBusy(false);
      activeTurnIdRef.current = null;
      setActiveTurnId(null);
      const client = clientRef.current;
      if (client) {
        void client.request<{ thread: CodexThread }>("thread/resume", { threadId }).then((response) => {
          if (clientRef.current !== client || activeThreadRef.current?.id !== threadId) return;
          setActiveThread(response.thread);
          activeThreadRef.current = response.thread;
          setStreamText("");
        }).catch((reason) => setError(toMessage(reason)));
      }
      return;
    }
    if (message.method === "serverRequest/resolved") {
      const requestId = message.params?.requestId;
      setApprovals((current) => current.filter((entry) => entry.request.id !== requestId));
      return;
    }
    if (message.method === "thread/name/updated" && threadId) {
      const name = typeof message.params?.name === "string" ? message.params.name : null;
      setThreads((current) => current.map((thread) => thread.id === threadId ? { ...thread, name } : thread));
    }
  }

  function updateVisibleThread(update: (thread: CodexThread) => CodexThread) {
    setActiveThread((current) => {
      if (!current) return current;
      const next = update(current);
      activeThreadRef.current = next;
      return next;
    });
  }

  async function handleServerRequest(client: GatewayAppServerClient, hostId: string, request: ServerRequest) {
    if (request.method === "currentTime/read") {
      await client.respond(request.id, { currentTimeAt: Math.floor(Date.now() / 1000) });
      return;
    }
    if (isApprovalRequest(request.method)) {
      setApprovals((current) => [{ request, client, hostId }, ...current]);
      return;
    }
    await client.respondError(request.id, -32601, `${request.method} はWeb UIでは未対応です`);
  }

  async function resolveApproval(entry: PendingApproval, accepted: boolean) {
    const { request, client } = entry;
    try {
      if (request.method === "item/commandExecution/requestApproval" || request.method === "item/fileChange/requestApproval") {
        await client.respond(request.id, { decision: accepted ? "accept" : "decline" });
      } else {
        await client.respond(request.id, { decision: accepted ? "approved" : "denied" });
      }
      setApprovals((current) => current.filter((candidate) => candidate !== entry));
    } catch (reason) {
      setError(toMessage(reason));
    }
  }

  if (restoring) {
    return <main className="web-splash"><LoaderCircle aria-label="セッションを確認中" /></main>;
  }

  if (!authenticated) {
    return (
      <main className="web-login">
        <form className="web-card" onSubmit={login}>
          <div className="web-login-mark"><TerminalSquare aria-hidden="true" /></div>
          <div>
            <p className="web-kicker">HUTONCODEX</p>
            <h1>Gatewayに接続</h1>
            <p>このGatewayに設定したパスワードを入力します。</p>
          </div>
          <label>パスワード<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" autoFocus /></label>
          <button type="submit" disabled={!password}>続ける</button>
          {error && <p role="alert" className="web-error"><CircleAlert aria-hidden="true" />{error}</p>}
        </form>
      </main>
    );
  }

  return (
    <main className="web-shell">
      <button className={`web-scrim ${sidebarOpen ? "visible" : ""}`} aria-label="サイドバーを閉じる" onClick={() => setSidebarOpen(false)} />
      <aside className={`web-sidebar ${sidebarOpen ? "open" : ""}`} aria-label="タスクナビゲーション">
        <div className="web-brand-wrap">
          <button className="web-brand" onClick={() => setHostMenuOpen((current) => !current)} aria-expanded={hostMenuOpen}>
            <span className="web-brand-mark"><TerminalSquare aria-hidden="true" /></span>
            <span>hutoncodex</span>
            <ChevronDown aria-hidden="true" />
          </button>
          <button className="web-sidebar-close" onClick={() => setSidebarOpen(false)} aria-label="サイドバーを閉じる"><X /></button>
          {hostMenuOpen && (
            <div className="web-host-menu">
              <div className="web-menu-heading"><span>Host</span><button onClick={() => void refreshHosts()} aria-label="Host一覧を更新"><RefreshCw /></button></div>
              {hosts.map((entry) => (
                <button className={host?.id === entry.id ? "selected" : ""} key={entry.id} onClick={() => void selectHost(entry)}>
                  <Monitor aria-hidden="true" />
                  <span><strong>{entry.displayName}</strong><small>{entry.lunaMax?.state === "available" ? "Luna Maxを利用可能" : "App Serverを利用可能"}</small></span>
                </button>
              ))}
              {!hosts.length && <p>起動中のHost Agentがありません。</p>}
              <button className="web-logout" onClick={() => void logout()}>Gatewayからログアウト</button>
            </div>
          )}
        </div>

        <div className="web-search">
          <Search aria-hidden="true" />
          <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="タスクを検索" aria-label="タスクを検索" />
        </div>

        <div className="web-sidebar-status">
          <span className={`web-status-dot ${status}`} />
          <span>{host ? host.displayName : "Host未選択"}</span>
          <small>{statusLabel(status, catalogLoading)}</small>
        </div>

        <nav className="web-thread-list" aria-label="タスク一覧">
          {groupedThreads.map((group) => (
            <section key={group.project}>
              <div className="web-project-heading"><Folder aria-hidden="true" /><span>{group.project}</span></div>
              {group.threads.map((thread) => (
                <button className={activeThread?.id === thread.id ? "active" : ""} key={thread.id} onClick={() => void openThread(thread.id)} title={threadTitle(thread)}>
                  <span>{threadTitle(thread)}</span>
                  {openingThreadId === thread.id ? <LoaderCircle className="spin" aria-label="読込中" /> : <small>{formatRelativeTime(thread.updatedAt)}</small>}
                </button>
              ))}
            </section>
          ))}
          {host && !catalogLoading && !groupedThreads.length && <p className="web-muted">一致するタスクはありません。</p>}
          {!host && <p className="web-muted">上のHostメニューから接続先を選択してください。</p>}
        </nav>
      </aside>

      <section className="web-workspace">
        <header className="web-workspace-header">
          <div>
            <button className="web-sidebar-toggle" onClick={() => setSidebarOpen(true)} aria-label="サイドバーを開く"><PanelLeft /></button>
            {activeThread?.cwd && <Folder aria-hidden="true" />}
            <strong>{activeThread ? threadTitle(activeThread) : host?.displayName || "Hostを選択"}</strong>
          </div>
          <span className={`web-connection-state ${busy ? "busy" : ""}`}>{busy ? "作業中" : statusLabel(status, catalogLoading)}</span>
        </header>

        {error && <div role="alert" className="web-banner"><CircleAlert aria-hidden="true" /><span>{error}</span><button onClick={() => setError("")} aria-label="エラーを閉じる"><X /></button></div>}

        <div className="web-conversation" ref={conversationRef} onScroll={(event) => {
          const element = event.currentTarget;
          followOutputRef.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
        }}>
          {!activeThread && <EmptyState host={host} loading={catalogLoading} />}
          {activeThread?.turns.flatMap((turn) => turn.items.map((item, index) => (
            <ThreadItemView item={item} key={`${turn.id}:${item.id || index}`} />
          )))}
          {streamText && <article className="web-message web-agent-message streaming"><ReactMarkdown remarkPlugins={[remarkGfm]}>{streamText}</ReactMarkdown><span className="web-stream-caret" /></article>}
          {approvals.filter((entry) => entry.hostId === host?.id).map((entry) => (
            <ApprovalCard entry={entry} key={`${entry.hostId}:${entry.request.method}:${entry.request.id}`} onResolve={resolveApproval} />
          ))}
          {busy && !streamText && <div className="web-thinking"><LoaderCircle className="spin" /><span>作業しています</span></div>}
        </div>

        <form className="web-composer-wrap" onSubmit={sendTurn}>
          <div className="web-composer">
            <textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} onKeyDown={handleComposerKeyDown} placeholder={activeThread ? "Codexに指示する" : "タスクを選択してください"} disabled={!activeThread} rows={1} aria-label="Codexへの指示" />
            <div className="web-composer-toolbar">
              <div className="web-model-controls">
                <label>
                  <span className="sr-only">モデル</span>
                  <select value={model} onChange={(event) => {
                    const nextModel = event.target.value;
                    const selected = models.find((entry) => entry.model === nextModel);
                    setModel(nextModel);
                    setEffort(selected?.defaultReasoningEffort || "");
                  }} disabled={!models.length}>
                    {models.map((entry) => <option key={entry.model} value={entry.model}>{entry.displayName || entry.model}</option>)}
                  </select>
                  <ChevronDown aria-hidden="true" />
                </label>
                {supportedEfforts.length > 0 && (
                  <label>
                    <span className="sr-only">推論レベル</span>
                    <select value={effort} onChange={(event) => setEffort(event.target.value)}>
                      {supportedEfforts.map((entry) => <option key={entry.reasoningEffort} value={entry.reasoningEffort}>{effortLabel(entry.reasoningEffort)}</option>)}
                    </select>
                    <ChevronDown aria-hidden="true" />
                  </label>
                )}
              </div>
              {busy ? (
                <button type="button" className="web-send web-stop" onClick={() => void interruptTurn()} disabled={!activeTurnId} aria-label="ターンを停止"><Square /></button>
              ) : (
                <button type="submit" className="web-send" disabled={!activeThread || !prompt.trim()} aria-label="送信"><Send /></button>
              )}
            </div>
          </div>
          <p>Enterで送信 · Shift+Enterで改行</p>
        </form>
      </section>
    </main>
  );
}

function EmptyState({ host, loading }: { host: GatewayHost | null; loading: boolean }) {
  return (
    <div className="web-empty">
      <span className="web-empty-icon">{loading ? <LoaderCircle className="spin" /> : <TerminalSquare />}</span>
      <h1>{loading ? "タスクを読み込んでいます" : host ? "タスクを選択" : "Hostに接続"}</h1>
      <p>{loading ? "App Serverからタスクとモデルを取得しています。" : host ? "左の一覧から既存タスクを開けます。" : "左上のHostメニューから、接続済みのHost Agentを選択してください。"}</p>
    </div>
  );
}

function ThreadItemView({ item }: { item: ThreadItem }) {
  if (item.type === "userMessage") {
    return <article className="web-message web-user-message"><p>{extractUserText(item)}</p></article>;
  }
  if (item.type === "agentMessage" || item.type === "plan") {
    return <article className="web-message web-agent-message"><ReactMarkdown remarkPlugins={[remarkGfm]}>{item.text || ""}</ReactMarkdown></article>;
  }
  if (item.type === "reasoning") {
    const summary = Array.isArray(item.summary) ? item.summary.join("\n") : "";
    return <details className="web-activity"><summary><ChevronRight />考えたこと</summary><pre>{summary || "詳細はありません"}</pre></details>;
  }
  if (item.type === "commandExecution") {
    return (
      <details className="web-activity" open={item.status !== "completed"}>
        <summary><TerminalSquare /><span>コマンドを実行</span><code>{item.command}</code><small>{activityStatus(item.status)}</small></summary>
        {item.cwd && <p className="web-activity-path">{item.cwd}</p>}
        {item.aggregatedOutput && <pre>{item.aggregatedOutput}</pre>}
      </details>
    );
  }
  if (item.type === "fileChange") {
    return (
      <details className="web-activity">
        <summary><Folder /><span>ファイルを変更</span><small>{activityStatus(item.status)}</small></summary>
        <pre>{JSON.stringify(item.changes ?? [], null, 2)}</pre>
      </details>
    );
  }
  if (item.type === "contextCompaction") {
    return <div className="web-context-note">コンテキストを整理しました</div>;
  }
  return (
    <details className="web-activity">
      <summary><ChevronRight /><span>{itemLabel(item.type)}</span><small>{activityStatus(item.status)}</small></summary>
      <pre>{safeJson(item)}</pre>
    </details>
  );
}

function ApprovalCard({ entry, onResolve }: { entry: PendingApproval; onResolve: (entry: PendingApproval, accepted: boolean) => Promise<void> }) {
  const params = entry.request.params;
  const command = typeof params.command === "string" ? params.command : null;
  const reason = typeof params.reason === "string" ? params.reason : null;
  const isFile = entry.request.method.includes("fileChange") || entry.request.method === "applyPatchApproval";
  return (
    <article className="web-approval">
      <div><CircleAlert /><span><strong>{isFile ? "ファイル変更の承認" : "コマンド実行の承認"}</strong><small>Codexが操作の許可を待っています</small></span></div>
      {(command || reason) && <pre>{command || reason}</pre>}
      <div className="web-approval-actions">
        <button onClick={() => void onResolve(entry, false)}>拒否</button>
        <button className="primary" onClick={() => void onResolve(entry, true)}>許可</button>
      </div>
    </article>
  );
}

export function groupThreads(threads: CodexThread[], query: string) {
  const normalized = query.trim().toLocaleLowerCase();
  const groups = new Map<string, CodexThread[]>();
  for (const thread of threads) {
    const searchable = `${threadTitle(thread)} ${thread.cwd}`.toLocaleLowerCase();
    if (normalized && !searchable.includes(normalized)) continue;
    const project = projectName(thread.cwd);
    const entries = groups.get(project) ?? [];
    entries.push(thread);
    groups.set(project, entries);
  }
  return [...groups.entries()]
    .map(([project, entries]) => ({ project, threads: entries.sort((a, b) => b.updatedAt - a.updatedAt) }))
    .sort((a, b) => (b.threads[0]?.updatedAt ?? 0) - (a.threads[0]?.updatedAt ?? 0));
}

export function threadTitle(thread: CodexThread) {
  return (thread.name || thread.preview || "名称未設定").trim() || "名称未設定";
}

export function projectName(cwd: string) {
  const parts = cwd.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts.at(-1) || "Workspace";
}

export function mergeTurnIntoThread(thread: CodexThread, turn: Turn) {
  const index = thread.turns.findIndex((entry) => entry.id === turn.id);
  if (index < 0) return { ...thread, turns: [...thread.turns, turn] };
  const turns = [...thread.turns];
  turns[index] = { ...turns[index], ...turn, items: turn.items.length ? turn.items : turns[index].items };
  return { ...thread, turns };
}

export function mergeItemIntoThread(thread: CodexThread, turnId: string, item: ThreadItem) {
  return {
    ...thread,
    turns: thread.turns.map((turn) => {
      if (turn.id !== turnId) return turn;
      const index = turn.items.findIndex((entry) => entry.id && entry.id === item.id);
      if (index < 0) return { ...turn, items: [...turn.items, item] };
      const items = [...turn.items];
      items[index] = { ...items[index], ...item };
      return { ...turn, items };
    }),
  };
}

function extractUserText(item: ThreadItem) {
  if (typeof item.text === "string") return item.text;
  if (!Array.isArray(item.content)) return "";
  return item.content.map((entry) => {
    if (typeof entry === "string") return entry;
    return "text" in entry && typeof entry.text === "string" ? entry.text : "";
  }).filter(Boolean).join("\n");
}

function isApprovalRequest(method: string) {
  return method === "item/commandExecution/requestApproval"
    || method === "item/fileChange/requestApproval"
    || method === "execCommandApproval"
    || method === "applyPatchApproval";
}

function statusLabel(status: string, catalogLoading = false) {
  if (catalogLoading) return "同期中";
  if (status === "connected" || status === "appServerReady") return "接続済み";
  if (status === "connecting" || status === "appServerStarting") return "接続中";
  if (status === "error") return "接続エラー";
  return "未接続";
}

function effortLabel(effort: string) {
  const labels: Record<string, string> = { none: "推論なし", minimal: "最小", low: "低", medium: "中", high: "高", xhigh: "最高", max: "最大" };
  return labels[effort] ?? effort;
}

function activityStatus(status: unknown) {
  if (status === "completed") return "完了";
  if (status === "failed") return "失敗";
  if (status === "inProgress") return "実行中";
  return typeof status === "string" ? status : "";
}

function itemLabel(type: string) {
  const labels: Record<string, string> = { mcpToolCall: "MCPツール", dynamicToolCall: "ツール", collabAgentToolCall: "サブエージェント", webSearch: "Web検索", imageView: "画像を表示", imageGeneration: "画像を生成", sleep: "待機" };
  return labels[type] ?? type;
}

function formatRelativeTime(timestamp: number) {
  const elapsed = Math.max(0, Date.now() / 1000 - timestamp);
  if (elapsed < 60) return "今";
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}分`;
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}時間`;
  if (elapsed < 604800) return `${Math.floor(elapsed / 86400)}日`;
  return new Date(timestamp * 1000).toLocaleDateString("ja-JP", { month: "numeric", day: "numeric" });
}

function safeJson(value: unknown) {
  try { return JSON.stringify(value, null, 2); } catch { return String(value); }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
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
