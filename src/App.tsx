import {
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Archive,
  Blocks,
  Camera,
  ChartNoAxesColumnIncreasing,
  Check,
  ChevronDown,
  ChevronRight,
  CircleStop,
  Clock3,
  Code2,
  FileCode2,
  Folder,
  GitPullRequest,
  Globe2,
  ImagePlus,
  Keyboard,
  LoaderCircle,
  Mic,
  Minus,
  MoreHorizontal,
  PanelLeft,
  Plus,
  QrCode,
  RefreshCw,
  Search,
  Send,
  Server,
  Shield,
  ShieldAlert,
  Sparkles,
  Square,
  SquarePen,
  Terminal,
  Wifi,
  WifiOff,
  Wrench,
  X,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onBackButtonPress } from "@tauri-apps/api/app";
import { BrowserQRCodeReader, type IScannerControls } from "@zxing/browser";
import { FormEvent, KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ConnectionSwitcher } from "./components/ConnectionSwitcher";
import { getCompactUsage, UsageSettings } from "./components/UsageSettings";
import { AppServerClient } from "./lib/appServer";
import { listAllThreads } from "./lib/catalogs";
import { deriveDiscordPresence, updateDiscordPresence } from "./lib/discordPresence";
import {
  isNearScrollBottom,
  shouldSubmitComposer,
  useModalFocus,
  useViewportMetrics,
} from "./lib/mobileUi";
import {
  DEFAULT_RUNTIME_CAPABILITIES,
  getRuntimeCapabilities,
  type RuntimeCapabilities,
} from "./lib/runtimeCapabilities";
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
} from "./lib/operationState";
import type {
  AccountRateLimitsResponse,
  AccountTokenUsageResponse,
  CodexModel,
  CodexThread,
  ConnectionStatus,
  InitializeResponse,
  ManagedConnection,
  ManagedConnectionMode,
  PairingProgress,
  ServerRequest,
  ThreadItem,
  Turn,
  WireMessage,
} from "./types";

type TransportState = "disconnected" | "connecting" | "connected";
type ConnectionMode = "manual" | "qr" | "advanced";
type ConnectionViewState = {
  activeThread: CodexThread | null;
  cwd: string;
  busy: boolean;
  prompt: string;
  threads: CodexThread[];
  models: CodexModel[];
  selectedModel: string;
  selectedEffort: string;
  catalogLoading: boolean;
  rateLimits: AccountRateLimitsResponse | null;
  tokenUsage: AccountTokenUsageResponse | null;
  usageLoading: boolean;
  usageError: string;
};

const DEFAULT_ENDPOINT = "ws://127.0.0.1:4500";
const PREVIEW_SCREEN = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("preview") : null;
const DESIGN_PREVIEW = ["active", "usage", "connections", "mobile", "mobile-connections"].includes(PREVIEW_SCREEN || "");
const USAGE_PREVIEW = PREVIEW_SCREEN === "usage";
const CONNECTIONS_PREVIEW = PREVIEW_SCREEN === "connections" || PREVIEW_SCREEN === "mobile-connections";
const MOBILE_PREVIEW = PREVIEW_SCREEN === "mobile" || PREVIEW_SCREEN === "mobile-connections";

const PREVIEW_MODELS: CodexModel[] = [
  {
    id: "gpt-5.6-sol",
    model: "gpt-5.6-sol",
    displayName: "GPT-5.6 Sol",
    description: "最も高性能なエージェント型コーディングモデル",
    hidden: false,
    isDefault: true,
    defaultReasoningEffort: "xhigh",
    supportedReasoningEfforts: ["low", "medium", "high", "xhigh"].map((reasoningEffort) => ({ reasoningEffort })),
  },
  {
    id: "gpt-5.6-terra",
    model: "gpt-5.6-terra",
    displayName: "GPT-5.6 Terra",
    description: "日常の開発作業向けのバランス型モデル",
    hidden: false,
    isDefault: false,
    defaultReasoningEffort: "medium",
    supportedReasoningEfforts: ["low", "medium", "high", "xhigh"].map((reasoningEffort) => ({ reasoningEffort })),
  },
];

const PREVIEW_ACTIVE_THREAD: CodexThread = {
  id: "preview-active",
  sessionId: "preview-session",
  preview: "Codex UIをさらに忠実にする",
  name: "Codex UIをさらに忠実にする",
  cwd: "C:\\Users\\hgzt23678\\Documents\\codexremote",
  modelProvider: "openai",
  createdAt: 1,
  updatedAt: 4,
  recencyAt: 4,
  status: { type: "idle" },
  turns: [
    {
      id: "preview-turn",
      status: "completed",
      error: null,
      startedAt: 1,
      completedAt: 2,
      durationMs: 72000,
      items: [
        { type: "userMessage", content: [{ type: "text", text: "さらにCodexのUIへ近づけて", text_elements: [] }] },
        { type: "reasoning", summary: ["現行Codexの画面構造と余白を比較しています。"] },
        {
          type: "agentMessage",
          text: "現行Codexとの差分を整理し、タイトルバー、サイドバー、コンポーザーの密度を揃えます。Pair接続とRemote App Serverの操作は維持します。",
        },
      ],
    },
  ],
};

const PREVIEW_THREADS: CodexThread[] = [
  PREVIEW_ACTIVE_THREAD,
  { ...PREVIEW_ACTIVE_THREAD, id: "preview-2", sessionId: "preview-2", name: "Pair接続を実装する", preview: "Pair接続を実装する", cwd: "C:\\Users\\hgzt23678\\Documents\\codexremote", turns: [] },
  { ...PREVIEW_ACTIVE_THREAD, id: "preview-3", sessionId: "preview-3", name: "リポジトリを確認する", preview: "リポジトリを確認する", cwd: "C:\\Users\\hgzt23678\\Documents\\cherrypick", turns: [] },
  { ...PREVIEW_ACTIVE_THREAD, id: "preview-4", sessionId: "preview-4", name: "パフォーマンスを最適化", preview: "パフォーマンスを最適化", cwd: "C:\\Users\\hgzt23678\\Documents\\pcbuild", turns: [] },
];

const PREVIEW_RATE_LIMITS: AccountRateLimitsResponse = {
  rateLimits: {
    limitId: "codex",
    limitName: null,
    primary: { usedPercent: 32, windowDurationMins: 300, resetsAt: Math.floor(Date.now() / 1000) + 2 * 60 * 60 + 26 * 60 },
    secondary: { usedPercent: 19, windowDurationMins: 10080, resetsAt: Math.floor(Date.now() / 1000) + 4 * 24 * 60 * 60 + 7 * 60 * 60 },
    credits: { hasCredits: true, unlimited: false, balance: "250" },
    planType: "plus",
    rateLimitReachedType: null,
  },
  rateLimitsByLimitId: null,
  rateLimitResetCredits: { availableCount: 1, credits: null },
};

const PREVIEW_TOKEN_USAGE: AccountTokenUsageResponse = {
  summary: {
    lifetimeTokens: 18429510,
    peakDailyTokens: 821430,
    longestRunningTurnSec: 927,
    currentStreakDays: 6,
    longestStreakDays: 18,
  },
  dailyUsageBuckets: [
    { startDate: "2026-07-26", tokens: 312420 },
    { startDate: "2026-07-27", tokens: 481250 },
    { startDate: "2026-07-28", tokens: 279840 },
    { startDate: "2026-07-29", tokens: 821430 },
    { startDate: "2026-07-30", tokens: 546900 },
    { startDate: "2026-07-31", tokens: 694280 },
    { startDate: "2026-08-01", tokens: 418760 },
  ],
};

const PREVIEW_CONNECTIONS: ManagedConnection[] = [
  {
    id: "preview-main",
    connectionId: 101,
    label: "remote-workspace",
    mode: "manual",
    state: "connected",
    serverInfo: { userAgent: "hutoncodex-preview", codexHome: "~/.codex", platformFamily: "windows", platformOs: "Windows" },
    createdAt: 4,
  },
  {
    id: "preview-devbox",
    connectionId: 102,
    label: "devbox-tokyo",
    mode: "qr",
    state: "connected",
    serverInfo: { userAgent: "hutoncodex-preview", codexHome: "/home/codex/.codex", platformFamily: "unix", platformOs: "Linux" },
    createdAt: 3,
  },
  {
    id: "preview-laptop",
    connectionId: 103,
    label: "personal-laptop",
    mode: "advanced",
    state: "connected",
    endpoint: "wss://laptop.example.test",
    serverInfo: { userAgent: "hutoncodex-preview", codexHome: "~/.codex", platformFamily: "macos", platformOs: "macOS" },
    createdAt: 2,
  },
  ...Array.from({ length: 7 }, (_, index): ManagedConnection => ({
    id: `preview-extra-${index + 1}`,
    connectionId: 104 + index,
    label: index === 6
      ? "非常に長い接続名を持つモバイル検証用リモートワークスペース"
      : `remote-workspace (${index + 2})`,
    mode: index % 2 === 0 ? "manual" : "advanced",
    state: "connected",
    endpoint: index % 2 === 0 ? undefined : `wss://remote-${index + 2}.example.test`,
    serverInfo: {
      userAgent: "hutoncodex-preview",
      codexHome: "~/.codex",
      platformFamily: "unix",
      platformOs: index % 2 === 0 ? "Android" : "Linux",
    },
    createdAt: 1 - index,
  })),
];

export default function App() {
  const clientRef = useRef<AppServerClient | null>(null);
  const clientsRef = useRef(new Map<string, AppServerClient>());
  const addingClientRef = useRef<AppServerClient | null>(null);
  const connectionsRef = useRef<ManagedConnection[]>(DESIGN_PREVIEW ? PREVIEW_CONNECTIONS : []);
  const connectionViewsRef = useRef(new Map<string, ConnectionViewState>());
  const pendingRequestsRef = useRef(new Map<string, ServerRequest[]>());
  const activeConnectionIdRef = useRef<string | null>(DESIGN_PREVIEW ? PREVIEW_CONNECTIONS[0].id : null);
  const activeThreadIdRef = useRef<string | null>(null);
  const threadOperationGateRef = useRef(new LatestOperationGate());
  const catalogGenerationsRef = useRef(new Map<string, number>());
  const usageGenerationsRef = useRef(new Map<string, number>());
  const disconnectingRef = useRef(new Set<string>());
  const messageEndRef = useRef<HTMLDivElement | null>(null);
  const conversationRef = useRef<HTMLElement | null>(null);
  const composerTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const sidebarRef = useRef<HTMLElement | null>(null);
  const connectDialogRef = useRef<HTMLFormElement | null>(null);
  const modelPickerRef = useRef<HTMLDivElement | null>(null);
  const qrVideoRef = useRef<HTMLVideoElement | null>(null);
  const qrControlsRef = useRef<IScannerControls | null>(null);
  const qrScanGenerationRef = useRef(0);
  const qrStartingRef = useRef(false);
  const closeAfterConnectionCancelRef = useRef(false);
  const presenceGenerationRef = useRef(0);
  const stickToBottomRef = useRef(true);
  const lastConversationThreadRef = useRef<string | null>(null);
  const viewport = useViewportMetrics();
  const [transportReady, setTransportReady] = useState(DESIGN_PREVIEW);
  const [transportState, setTransportState] = useState<TransportState>(DESIGN_PREVIEW ? "connected" : "disconnected");
  const [endpoint, setEndpoint] = useState(
    () => localStorage.getItem("codexRemote.endpoint") || DEFAULT_ENDPOINT,
  );
  const [token, setToken] = useState("");
  const [connectionMode, setConnectionMode] = useState<ConnectionMode>("manual");
  const [pairCode, setPairCode] = useState("");
  const [qrValue, setQrValue] = useState("");
  const [pairingProgress, setPairingProgress] = useState("");
  const [pairPrepared, setPairPrepared] = useState(false);
  const [deviceAuthPrompt, setDeviceAuthPrompt] = useState<Pick<PairingProgress, "verificationUrl" | "userCode"> | null>(null);
  const [qrScanning, setQrScanning] = useState(false);
  const [qrStarting, setQrStarting] = useState(false);
  const [connectionLabel, setConnectionLabel] = useState(DESIGN_PREVIEW ? "remote-workspace" : "");
  const [serverInfo, setServerInfo] = useState<InitializeResponse | null>(DESIGN_PREVIEW ? { userAgent: "hutoncodex-preview", codexHome: "~/.codex", platformFamily: "windows", platformOs: "Windows" } : null);
  const [threads, setThreads] = useState<CodexThread[]>(DESIGN_PREVIEW ? PREVIEW_THREADS : []);
  const [activeThread, setActiveThread] = useState<CodexThread | null>(DESIGN_PREVIEW ? PREVIEW_ACTIVE_THREAD : null);
  const [models, setModels] = useState<CodexModel[]>(DESIGN_PREVIEW ? PREVIEW_MODELS : []);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [selectedModel, setSelectedModel] = useState(DESIGN_PREVIEW ? PREVIEW_MODELS[0].model : "");
  const [selectedEffort, setSelectedEffort] = useState(DESIGN_PREVIEW ? "xhigh" : "");
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const [cwd, setCwd] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [search, setSearch] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => !MOBILE_PREVIEW && !viewport.compact);
  const [connectOpen, setConnectOpen] = useState(!DESIGN_PREVIEW);
  const [connectionManagerOpen, setConnectionManagerOpen] = useState(CONNECTIONS_PREVIEW);
  const [connectionAdding, setConnectionAdding] = useState(false);
  const [connectionCancelling, setConnectionCancelling] = useState(false);
  const [connectError, setConnectError] = useState("");
  const [connections, setConnections] = useState<ManagedConnection[]>(DESIGN_PREVIEW ? PREVIEW_CONNECTIONS : []);
  const [activeConnectionId, setActiveConnectionId] = useState<string | null>(DESIGN_PREVIEW ? PREVIEW_CONNECTIONS[0].id : null);
  const [toast, setToast] = useState("");
  const [serverRequests, setServerRequests] = useState<ServerRequest[]>([]);
  const [usageOpen, setUsageOpen] = useState(USAGE_PREVIEW);
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageError, setUsageError] = useState("");
  const [rateLimits, setRateLimits] = useState<AccountRateLimitsResponse | null>(DESIGN_PREVIEW ? PREVIEW_RATE_LIMITS : null);
  const [tokenUsage, setTokenUsage] = useState<AccountTokenUsageResponse | null>(DESIGN_PREVIEW ? PREVIEW_TOKEN_USAGE : null);
  const [hasNewMessages, setHasNewMessages] = useState(false);
  const [runtimeCapabilities, setRuntimeCapabilities] = useState<RuntimeCapabilities>(DEFAULT_RUNTIME_CAPABILITIES);

  const overlayOpen = connectOpen || connectionManagerOpen || usageOpen;
  useModalFocus(sidebarRef, sidebarOpen && viewport.compact && !overlayOpen);
  useModalFocus(connectDialogRef, connectOpen);

  useEffect(() => {
    activeThreadIdRef.current = activeThread?.id ?? null;
  }, [activeThread?.id]);

  useEffect(() => {
    connectionsRef.current = connections;
  }, [connections]);

  useEffect(() => {
    if (!DESIGN_PREVIEW) {
      const tauriAvailable = "__TAURI_INTERNALS__" in window;
      setTransportReady(tauriAvailable);
      if (!tauriAvailable) setConnectError("この画面は Tauri デスクトップアプリ内で使用してください");
    }
    return () => {
      addingClientRef.current?.dispose();
      addingClientRef.current = null;
      for (const client of clientsRef.current.values()) client.dispose();
      clientsRef.current.clear();
      clientRef.current = null;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void getRuntimeCapabilities()
      .then((capabilities) => {
        if (disposed) return;
        setRuntimeCapabilities(capabilities);
        document.documentElement.classList.toggle("tauri-mobile", capabilities.mobile);
        if (!capabilities.pairingSupported) setConnectionMode("advanced");
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      document.documentElement.classList.remove("tauri-mobile");
    };
  }, []);

  useEffect(() => {
    if (!MOBILE_PREVIEW) return;
    document.body.classList.add("mobile-preview-body");
    return () => document.body.classList.remove("mobile-preview-body");
  }, []);

  useEffect(() => {
    activeConnectionIdRef.current = activeConnectionId;
  }, [activeConnectionId]);

  useEffect(() => {
    if (connectOpen && connectionMode === "qr") return;
    stopQrCamera();
  }, [connectOpen, connectionMode]);

  useEffect(() => {
    const threadId = activeThread?.id ?? null;
    if (lastConversationThreadRef.current !== threadId) {
      lastConversationThreadRef.current = threadId;
      stickToBottomRef.current = true;
      setHasNewMessages(false);
    }
    if (stickToBottomRef.current) {
      messageEndRef.current?.scrollIntoView({ behavior: "auto", block: "end" });
    } else if (activeThread?.turns?.length || serverRequests.length) {
      setHasNewMessages(true);
    }
  }, [activeThread?.id, activeThread?.turns, serverRequests.length]);

  useEffect(() => {
    const textarea = composerTextareaRef.current;
    if (!textarea) return;
    textarea.style.height = "auto";
    const maximum = viewport.phone ? 150 : 210;
    textarea.style.height = `${Math.min(textarea.scrollHeight, maximum)}px`;
  }, [prompt, viewport.phone, viewport.keyboardOpen]);

  useEffect(() => {
    const stopWhenHidden = () => {
      if (document.hidden) stopQrCamera();
    };
    const stopOnPageHide = () => stopQrCamera();
    document.addEventListener("visibilitychange", stopWhenHidden);
    window.addEventListener("pagehide", stopOnPageHide);
    return () => {
      document.removeEventListener("visibilitychange", stopWhenHidden);
      window.removeEventListener("pagehide", stopOnPageHide);
    };
  }, []);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(""), 5000);
    return () => clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    if (DESIGN_PREVIEW || !runtimeCapabilities.discordPresenceSupported || !("__TAURI_INTERNALS__" in window)) return;
    const presence = deriveDiscordPresence({
      connectionAdding,
      connectionMode,
      connectionError: Boolean(connectError),
      connected: transportState === "connected",
      busy,
      hasSelectedTask: Boolean(activeThread?.id),
      taskName: activeThread?.name || activeThread?.preview || null,
      pendingMethod: serverRequests[0]?.method || null,
    });
    void updateDiscordPresence({
      generation: ++presenceGenerationRef.current,
      ...presence,
    }).catch(() => undefined);
  }, [
    activeThread?.id,
    activeThread?.name,
    activeThread?.preview,
    busy,
    connectError,
    connectionAdding,
    connectionMode,
    serverRequests,
    transportState,
    runtimeCapabilities.discordPresenceSupported,
  ]);

  useEffect(() => {
    if (!modelPickerOpen) return;
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!modelPickerRef.current?.contains(event.target as Node)) setModelPickerOpen(false);
    };
    document.addEventListener("pointerdown", closeOnPointerDown);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
    };
  }, [modelPickerOpen]);

  useEffect(() => {
    const closeTopLayer = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (dismissTopLayer()) event.preventDefault();
    };
    document.addEventListener("keydown", closeTopLayer);
    return () => document.removeEventListener("keydown", closeTopLayer);
  }, [connectOpen, connectionAdding, usageOpen, connectionManagerOpen, modelPickerOpen, sidebarOpen, viewport.compact]);

  useEffect(() => {
    if (!runtimeCapabilities.mobile || !("__TAURI_INTERNALS__" in window)) return;
    let disposed = false;
    let unregister: (() => Promise<void>) | undefined;
    void onBackButtonPress(() => {
      if (!dismissTopLayer()) void getCurrentWindow().close();
    }).then((listener) => {
      if (disposed) void listener.unregister();
      else unregister = () => listener.unregister();
    }).catch(() => undefined);
    return () => {
      disposed = true;
      void unregister?.();
    };
  }, [runtimeCapabilities.mobile, connectOpen, connectionAdding, usageOpen, connectionManagerOpen, modelPickerOpen, sidebarOpen, viewport.compact]);

  const visibleThreads = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    if (!query) return threads;
    return threads.filter((thread) =>
      [thread.name, thread.preview, thread.cwd]
        .filter(Boolean)
        .some((value) => String(value).toLocaleLowerCase().includes(query)),
    );
  }, [search, threads]);
  const threadGroups = useMemo(() => groupThreads(visibleThreads), [visibleThreads]);

  const activeModel = models.find((model) => model.model === selectedModel);
  const efforts = activeModel?.supportedReasoningEfforts ?? [];
  const compactUsage = useMemo(() => getCompactUsage(rateLimits), [rateLimits]);
  const connectDialogLocked = connectionAdding || qrStarting;

  function selectComposerModel(model: CodexModel) {
    const effort = model.supportedReasoningEfforts.some(
      (item) => item.reasoningEffort === selectedEffort,
    )
      ? selectedEffort
      : model.defaultReasoningEffort || model.supportedReasoningEfforts[0]?.reasoningEffort || "";
    const profileId = activeConnectionIdRef.current;
    if (profileId) setConnectionModelSelection(profileId, model.model, effort);
    else {
      setSelectedModel(model.model);
      setSelectedEffort(effort);
    }
    setModelPickerOpen(false);
  }

  function getConnectionView(profileId: string): ConnectionViewState {
    return connectionViewsRef.current.get(profileId) ?? {
      activeThread: DESIGN_PREVIEW ? PREVIEW_ACTIVE_THREAD : null,
      cwd: DESIGN_PREVIEW ? PREVIEW_ACTIVE_THREAD.cwd : "",
      busy: false,
      prompt: "",
      threads: DESIGN_PREVIEW ? PREVIEW_THREADS : [],
      models: DESIGN_PREVIEW ? PREVIEW_MODELS : [],
      selectedModel: DESIGN_PREVIEW ? PREVIEW_MODELS[0].model : "",
      selectedEffort: DESIGN_PREVIEW ? "xhigh" : "",
      catalogLoading: false,
      rateLimits: DESIGN_PREVIEW ? PREVIEW_RATE_LIMITS : null,
      tokenUsage: DESIGN_PREVIEW ? PREVIEW_TOKEN_USAGE : null,
      usageLoading: false,
      usageError: "",
    };
  }

  function setConnectionThreads(
    profileId: string,
    updater: (current: CodexThread[]) => CodexThread[],
  ) {
    const view = getConnectionView(profileId);
    const next = updater(view.threads);
    connectionViewsRef.current.set(profileId, { ...view, threads: next });
    if (activeConnectionIdRef.current === profileId) setThreads(next);
  }

  function setConnectionModelSelection(profileId: string, model: string, effort: string) {
    const view = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, {
      ...view,
      selectedModel: model,
      selectedEffort: effort,
    });
    if (activeConnectionIdRef.current === profileId) {
      setSelectedModel(model);
      setSelectedEffort(effort);
    }
  }

  function setConnectionPrompt(profileId: string, value: string) {
    const view = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, { ...view, prompt: value });
    if (activeConnectionIdRef.current === profileId) setPrompt(value);
  }

  function setConnectionCwd(profileId: string, value: string) {
    const view = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, { ...view, cwd: value });
    if (activeConnectionIdRef.current === profileId) setCwd(value);
  }

  function restoreConnectionPromptIfEmpty(profileId: string, value: string) {
    const view = getConnectionView(profileId);
    const restored = restoreDraftAfterFailure(view.prompt, value);
    if (restored === view.prompt) return;
    connectionViewsRef.current.set(profileId, { ...view, prompt: restored });
    if (activeConnectionIdRef.current === profileId) {
      setPrompt((current) => restoreDraftAfterFailure(current, value));
    }
  }

  function updatePrompt(value: string) {
    const profileId = activeConnectionIdRef.current;
    if (profileId) setConnectionPrompt(profileId, value);
    else setPrompt(value);
  }

  function setConnectionThread(
    profileId: string,
    thread: CodexThread | null,
    nextCwd?: string,
  ) {
    const current = getConnectionView(profileId);
    const next = {
      ...current,
      activeThread: thread,
      cwd: nextCwd ?? current.cwd,
    };
    connectionViewsRef.current.set(profileId, next);
    if (activeConnectionIdRef.current !== profileId) return;
    activeThreadIdRef.current = thread?.id ?? null;
    setActiveThread(thread);
    if (nextCwd !== undefined) setCwd(nextCwd);
  }

  function updateConnectionThread(
    profileId: string,
    threadId: string,
    updater: (thread: CodexThread) => CodexThread,
  ) {
    const view = getConnectionView(profileId);
    if (view.activeThread?.id !== threadId) return;
    const nextThread = updater(view.activeThread);
    connectionViewsRef.current.set(profileId, { ...view, activeThread: nextThread });
    if (activeConnectionIdRef.current === profileId) {
      activeThreadIdRef.current = nextThread.id;
      setActiveThread(nextThread);
    }
  }

  function setConnectionBusy(profileId: string, value: boolean) {
    const view = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, { ...view, busy: value });
    if (activeConnectionIdRef.current === profileId) setBusy(value);
  }

  function setPendingRequestsForConnection(
    profileId: string,
    updater: (current: ServerRequest[]) => ServerRequest[],
  ) {
    const current = pendingRequestsRef.current.get(profileId) ?? [];
    const next = updater(current);
    pendingRequestsRef.current.set(profileId, next);
    if (activeConnectionIdRef.current === profileId) setServerRequests(next);
    setConnections((connections) => connections.map((connection) => connection.id === profileId
      ? { ...connection, detail: next.length ? `${next.length}件の操作待ち` : undefined }
      : connection));
  }

  function isActiveClient(profileId: string, client: AppServerClient) {
    return activeConnectionIdRef.current === profileId && clientRef.current === client;
  }

  function closeConnectDialog() {
    if (connectionAdding || qrStartingRef.current) return;
    stopQrCamera();
    setConnectOpen(false);
    if (connectionsRef.current.length) setConnectionManagerOpen(true);
  }

  function dismissTopLayer() {
    if (connectOpen) {
      if (connectionAdding || qrStartingRef.current) return false;
      closeConnectDialog();
      return true;
    }
    if (usageOpen) {
      setUsageOpen(false);
      return true;
    }
    if (connectionManagerOpen) {
      setConnectionManagerOpen(false);
      return true;
    }
    if (modelPickerOpen) {
      setModelPickerOpen(false);
      return true;
    }
    if (sidebarOpen && viewport.compact) {
      setSidebarOpen(false);
      return true;
    }
    return false;
  }

  function changeConnectionMode(mode: ConnectionMode) {
    if (connectionAdding || qrStartingRef.current || mode === connectionMode) return;
    if (mode !== "advanced" && !runtimeCapabilities.pairingSupported) return;
    stopQrCamera();
    setConnectError("");
    setPairingProgress("");
    setDeviceAuthPrompt(null);
    setConnectionMode(mode);
  }

  function createManagedClient(profileId: string, mode: ManagedConnectionMode) {
    let client: AppServerClient;
    client = new AppServerClient({
      onNotification: (message) => handleNotification(profileId, message),
      onServerRequest: (request) => {
        if (request.method === "currentTime/read") {
          void client.respond(request.id, { currentTimeAt: Math.floor(Date.now() / 1000) })
            .catch((error) => {
              if (activeConnectionIdRef.current === profileId) setToast(errorMessage(error));
            });
          return;
        }
        setPendingRequestsForConnection(profileId, (current) => [
          ...current.filter((item) => item.id !== request.id),
          request,
        ]);
      },
      onStatus: (status) => handleConnectionStatus(profileId, status),
      onPairingProgress: (progress) => {
        setPairingProgress(progress.detail);
        setDeviceAuthPrompt(progress.verificationUrl && progress.userCode
          ? { verificationUrl: progress.verificationUrl, userCode: progress.userCode }
          : null);
      },
      onConnectionPhase: (phase) => {
        setDeviceAuthPrompt(null);
        setPairingProgress(
          phase === "initializing"
            ? "App Serverを初期化しています"
            : mode === "advanced"
              ? "App Serverへ接続しています"
              : mode === "qr"
                ? "QR Pair接続を開始しています"
                : "Pair接続を開始しています",
        );
      },
    });
    return client;
  }

  async function addConnection(mode: ManagedConnectionMode) {
    if (!transportReady || connectionAdding) return;
    if (mode !== "advanced" && !runtimeCapabilities.pairingSupported) {
      setConnectError("この端末では公式Pair用のOS保護端末鍵を利用できません。上級者向け接続を使用してください。");
      return;
    }
    if (mode !== "advanced" && !pairPrepared) {
      setConnectError("先にCodex認証と端末登録を準備してください");
      return;
    }
    const code = mode === "manual" ? pairCode.trim() : qrValue.trim();
    if (mode !== "advanced" && !code) return;
    const profileId = crypto.randomUUID();
    const client = createManagedClient(profileId, mode);
    addingClientRef.current = client;
    closeAfterConnectionCancelRef.current = false;
    setConnectError("");
    setConnectionAdding(true);
    setDeviceAuthPrompt(null);
    setPairingProgress(mode === "advanced" ? "App Serverへ接続しています" : "Pair接続を開始しています");
    stopQrCamera();
    try {
      await client.mount();
      const info = mode === "advanced"
        ? await client.connect(endpoint.trim(), token)
        : await client.connectPair(code, mode);
      if (mode === "advanced") localStorage.setItem("codexRemote.endpoint", endpoint.trim());
      const profile: ManagedConnection = {
        id: profileId,
        connectionId: client.getConnectionId(),
        label: mode === "advanced"
          ? uniqueConnectionLabel(endpointHost(endpoint), connectionsRef.current)
          : defaultConnectionLabel(info, connectionsRef.current),
        mode,
        state: "connected",
        endpoint: mode === "advanced" ? endpoint.trim() : undefined,
        serverInfo: info,
        createdAt: Date.now(),
      };
      clientsRef.current.set(profileId, client);
      if (!pendingRequestsRef.current.has(profileId)) pendingRequestsRef.current.set(profileId, []);
      connectionViewsRef.current.set(profileId, {
        activeThread: null,
        cwd: "",
        busy: false,
        prompt: "",
        threads: [],
        models: [],
        selectedModel: "",
        selectedEffort: "",
        catalogLoading: false,
        rateLimits: null,
        tokenUsage: null,
        usageLoading: false,
        usageError: "",
      });
      setConnections((current) => {
        const next = [profile, ...current];
        connectionsRef.current = next;
        return next;
      });
      setPairCode("");
      setQrValue("");
      setToken("");
      setPairingProgress("");
      setDeviceAuthPrompt(null);
      setConnectOpen(false);
      await activateConnection(profile, client);
    } catch (error) {
      client.dispose();
      clientsRef.current.delete(profileId);
      pendingRequestsRef.current.delete(profileId);
      connectionViewsRef.current.delete(profileId);
      setPairingProgress("");
      setDeviceAuthPrompt(null);
      setConnectError(errorMessage(error));
    } finally {
      if (addingClientRef.current === client) addingClientRef.current = null;
      setConnectionAdding(false);
      setConnectionCancelling(false);
      if (closeAfterConnectionCancelRef.current) {
        closeAfterConnectionCancelRef.current = false;
        setConnectOpen(false);
        if (connectionsRef.current.length) setConnectionManagerOpen(true);
      }
    }
  }

  async function preparePairAuthentication() {
    if (!transportReady || connectionAdding || pairPrepared) return;
    const client = createManagedClient(`pair-preparation-${crypto.randomUUID()}`, connectionMode);
    addingClientRef.current = client;
    closeAfterConnectionCancelRef.current = false;
    setConnectError("");
    setConnectionAdding(true);
    setDeviceAuthPrompt(null);
    setPairingProgress("Codex認証と端末登録を準備しています");
    stopQrCamera();
    try {
      await client.mount();
      await client.preparePair();
      setPairPrepared(true);
      setPairingProgress("");
      setDeviceAuthPrompt(null);
    } catch (error) {
      setPairPrepared(false);
      setPairingProgress("");
      setDeviceAuthPrompt(null);
      setConnectError(errorMessage(error));
    } finally {
      client.dispose();
      if (addingClientRef.current === client) addingClientRef.current = null;
      setConnectionAdding(false);
      setConnectionCancelling(false);
      if (closeAfterConnectionCancelRef.current) {
        closeAfterConnectionCancelRef.current = false;
        setConnectOpen(false);
        if (connectionsRef.current.length) setConnectionManagerOpen(true);
      }
    }
  }

  async function cancelAddingConnection() {
    const client = addingClientRef.current;
    if (!client || connectionCancelling) return;
    closeAfterConnectionCancelRef.current = true;
    setConnectionCancelling(true);
    setPairingProgress("接続をキャンセルしています");
    try {
      await client.cancelConnectionAttempt();
    } catch (error) {
      closeAfterConnectionCancelRef.current = false;
      setConnectionCancelling(false);
      setConnectError(errorMessage(error));
    }
  }

  async function connectAdvanced(event?: FormEvent) {
    event?.preventDefault();
    await addConnection("advanced");
  }

  async function connectPair(kind: "manual" | "qr", event?: FormEvent) {
    event?.preventDefault();
    await addConnection(kind);
  }

  function submitConnection(event: FormEvent) {
    if (connectionMode === "advanced") void connectAdvanced(event);
    else void connectPair(connectionMode, event);
  }

  async function startQrCamera() {
    if (qrStartingRef.current) return;
    stopQrCamera();
    const generation = ++qrScanGenerationRef.current;
    let detected = false;
    setConnectError("");
    setQrScanning(true);
    qrStartingRef.current = true;
    setQrStarting(true);
    try {
      const reader = new BrowserQRCodeReader();
      const controls = await reader.decodeFromVideoDevice(
        undefined,
        qrVideoRef.current ?? undefined,
        (result) => {
          if (!result) return;
          detected = true;
          setQrValue(result.getText());
          setQrScanning(false);
          qrStartingRef.current = false;
          setQrStarting(false);
          qrControlsRef.current?.stop();
          qrControlsRef.current = null;
        },
      );
      if (detected || generation !== qrScanGenerationRef.current) {
        controls.stop();
        return;
      }
      qrStartingRef.current = false;
      setQrStarting(false);
      qrControlsRef.current = controls;
    } catch (error) {
      if (generation === qrScanGenerationRef.current) {
        qrStartingRef.current = false;
        setQrStarting(false);
        setQrScanning(false);
        setConnectError(`カメラを開始できません: ${errorMessage(error)}`);
      }
    }
  }

  function stopQrCamera() {
    qrScanGenerationRef.current += 1;
    qrStartingRef.current = false;
    setQrStarting(false);
    qrControlsRef.current?.stop();
    qrControlsRef.current = null;
    setQrScanning(false);
  }

  async function readQrImage(file?: File) {
    if (!file) return;
    setConnectError("");
    const url = URL.createObjectURL(file);
    try {
      const result = await new BrowserQRCodeReader().decodeFromImageUrl(url);
      setQrValue(result.getText());
    } catch (error) {
      setConnectError(`画像からQRコードを読み取れません: ${errorMessage(error)}`);
    } finally {
      URL.revokeObjectURL(url);
    }
  }

  function clearActiveConnection() {
    threadOperationGateRef.current.invalidate();
    clientRef.current = null;
    activeConnectionIdRef.current = null;
    setActiveConnectionId(null);
    setModelPickerOpen(false);
    setTransportState("disconnected");
    setServerInfo(null);
    setConnectionLabel("");
    activeThreadIdRef.current = null;
    setActiveThread(null);
    setBusy(false);
    setThreads([]);
    setModels([]);
    setSelectedModel("");
    setSelectedEffort("");
    setCatalogLoading(false);
    setServerRequests([]);
    setUsageOpen(false);
    setRateLimits(null);
    setTokenUsage(null);
    setUsageError("");
    setUsageLoading(false);
    setPrompt("");
  }

  async function activateConnection(profile: ManagedConnection, client = clientsRef.current.get(profile.id) || null) {
    threadOperationGateRef.current.invalidate();
    activeConnectionIdRef.current = profile.id;
    clientRef.current = client;
    setActiveConnectionId(profile.id);
    setConnectionLabel(profile.label);
    setServerInfo(profile.serverInfo);
    setTransportState(profile.state === "connected" ? "connected" : profile.state === "connecting" ? "connecting" : "disconnected");
    setConnectionManagerOpen(false);
    const pendingCount = pendingRequestsRef.current.get(profile.id)?.length ?? 0;
    setConnections((current) => current.map((connection) => connection.id === profile.id
      ? { ...connection, detail: pendingCount ? `${pendingCount}件の操作待ち` : undefined }
      : connection));
    setConnectOpen(false);
    setModelPickerOpen(false);
    const view = getConnectionView(profile.id);
    activeThreadIdRef.current = view.activeThread?.id ?? null;
    setActiveThread(view.activeThread);
    setCwd(view.cwd);
    setBusy(view.busy);
    setPrompt(view.prompt);
    setThreads(view.threads);
    setCatalogLoading(view.catalogLoading);
    setModels(view.models);
    setSelectedModel(view.selectedModel);
    setSelectedEffort(view.selectedEffort);
    setServerRequests(pendingRequestsRef.current.get(profile.id) || []);
    setRateLimits(view.rateLimits);
    setTokenUsage(view.tokenUsage);
    setUsageLoading(view.usageLoading);
    setUsageError(view.usageError);
    closeSidebarOnNarrowScreen();
    if (client && profile.state === "connected") {
      client.recordPhase("ui_operable");
      const catalogs = refreshCatalogs(client, profile.id, true);
      void refreshUsage(client, profile.id, true);
      await catalogs;
    }
  }

  async function switchConnection(profileId: string) {
    const profile = connectionsRef.current.find((connection) => connection.id === profileId);
    if (!profile) return;
    await activateConnection(profile);
  }

  async function disconnectConnection(profileId: string) {
    if (disconnectingRef.current.has(profileId)) return;
    disconnectingRef.current.add(profileId);
    const client = clientsRef.current.get(profileId);
    const wasActive = activeConnectionIdRef.current === profileId;
    const remaining = removeConnection(connectionsRef.current, profileId);
    connectionsRef.current = remaining;
    setConnections((current) => removeConnection(current, profileId));
    clientsRef.current.delete(profileId);
    pendingRequestsRef.current.delete(profileId);
    connectionViewsRef.current.delete(profileId);
    catalogGenerationsRef.current.delete(profileId);
    usageGenerationsRef.current.delete(profileId);
    if (wasActive) {
      const next = remaining.find((connection) => connection.state === "connected") || remaining[0];
      if (next) void activateConnection(next);
      else {
        clearActiveConnection();
        setConnectionManagerOpen(false);
        setConnectOpen(true);
      }
    }
    try {
      await client?.disconnect();
    } catch (error) {
      setToast(errorMessage(error));
    } finally {
      client?.dispose();
      disconnectingRef.current.delete(profileId);
    }
  }

  async function refreshCatalogs(
    client = clientRef.current,
    profileId = activeConnectionIdRef.current,
    measure = false,
  ) {
    if (!client || !profileId) return;
    const generation = (catalogGenerationsRef.current.get(profileId) ?? 0) + 1;
    catalogGenerationsRef.current.set(profileId, generation);
    const startingView = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, { ...startingView, catalogLoading: true });
    if (profileId === activeConnectionIdRef.current) setCatalogLoading(true);
    try {
      const threadsRequest = listAllThreads(client).then((value) => {
        if (measure) client.recordPhase("thread_list_completed");
        return value;
      });
      const modelsRequest = client.request<{ data: CodexModel[] }>("model/list", {
          limit: 100,
          includeHidden: false,
        }).then((value) => {
          if (measure) client.recordPhase("model_list_completed");
          return value;
        });
      const [allThreads, modelPage] = await Promise.all([
        threadsRequest,
        modelsRequest,
      ]);
      if (measure) client.recordPhase("catalogs_completed");
      if (catalogGenerationsRef.current.get(profileId) !== generation
        || !clientsRef.current.has(profileId)) return;
      const view = getConnectionView(profileId);
      const defaultModel = modelPage.data.find((model) => model.isDefault) ?? modelPage.data[0];
      const selected = modelPage.data.some((model) => model.model === view.selectedModel)
        ? view.selectedModel
        : defaultModel?.model ?? "";
      const selectedEntry = modelPage.data.find((model) => model.model === selected);
      const effort = selectedEntry?.supportedReasoningEfforts.some(
        (item) => item.reasoningEffort === view.selectedEffort,
      )
        ? view.selectedEffort
        : selectedEntry?.defaultReasoningEffort
          || selectedEntry?.supportedReasoningEfforts[0]?.reasoningEffort
          || "";
      const next = {
        ...view,
        threads: allThreads,
        models: modelPage.data,
        selectedModel: selected,
        selectedEffort: effort,
        catalogLoading: false,
      };
      connectionViewsRef.current.set(profileId, next);
      if (profileId === activeConnectionIdRef.current && clientRef.current === client) {
        setThreads(next.threads);
        setModels(next.models);
        setSelectedModel(next.selectedModel);
        setSelectedEffort(next.selectedEffort);
        setCatalogLoading(false);
      }
    } catch (error) {
      if (profileId === activeConnectionIdRef.current) setToast(errorMessage(error));
    } finally {
      if (catalogGenerationsRef.current.get(profileId) === generation) {
        const view = getConnectionView(profileId);
        connectionViewsRef.current.set(profileId, { ...view, catalogLoading: false });
        if (profileId === activeConnectionIdRef.current) setCatalogLoading(false);
      }
    }
  }

  async function refreshUsage(
    client = clientRef.current,
    profileId = activeConnectionIdRef.current,
    measure = false,
  ) {
    if (DESIGN_PREVIEW) return;
    if (!client || !profileId) return;
    const generation = (usageGenerationsRef.current.get(profileId) ?? 0) + 1;
    usageGenerationsRef.current.set(profileId, generation);
    const startingView = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, {
      ...startingView,
      usageLoading: true,
      usageError: "",
    });
    if (profileId === activeConnectionIdRef.current) {
      setUsageLoading(true);
      setUsageError("");
    }
    const [limitsResult, usageResult] = await Promise.allSettled([
      client.request<AccountRateLimitsResponse>("account/rateLimits/read", undefined, 15_000),
      client.request<AccountTokenUsageResponse>("account/usage/read", undefined, 15_000),
    ]);
    if (measure) client.recordPhase("usage_completed");
    if (usageGenerationsRef.current.get(profileId) !== generation
      || !clientsRef.current.has(profileId)) return;
    const view = getConnectionView(profileId);
    const next = {
      ...view,
      rateLimits: limitsResult.status === "fulfilled" ? limitsResult.value : view.rateLimits,
      tokenUsage: usageResult.status === "fulfilled" ? usageResult.value : view.tokenUsage,
      usageLoading: false,
      usageError: limitsResult.status === "rejected" && usageResult.status === "rejected"
        ? errorMessage(limitsResult.reason)
        : "",
    };
    connectionViewsRef.current.set(profileId, next);
    if (profileId === activeConnectionIdRef.current && clientRef.current === client) {
      setRateLimits(next.rateLimits);
      setTokenUsage(next.tokenUsage);
      setUsageError(next.usageError);
      setUsageLoading(false);
    }
  }

  async function openThread(thread: CodexThread) {
    const client = clientRef.current;
    const profileId = activeConnectionIdRef.current;
    if (!client || !profileId || busy) return;
    const token = threadOperationGateRef.current.begin(profileId, thread.id);
    setConnectionThread(profileId, { ...thread, turns: thread.turns ?? [] }, thread.cwd || "");
    closeSidebarOnNarrowScreen();
    try {
      const response = await client.request<{
        thread: CodexThread;
        model: string;
        reasoningEffort?: string | null;
      }>("thread/resume", { threadId: thread.id });
      if (!threadOperationGateRef.current.isCurrent(
        token,
        activeConnectionIdRef.current,
        activeThreadIdRef.current,
      ) || clientRef.current !== client) return;
      setConnectionThread(profileId, response.thread, response.thread.cwd || thread.cwd || "");
      const nextModel = response.model || getConnectionView(profileId).selectedModel;
      const nextEffort = response.reasoningEffort || getConnectionView(profileId).selectedEffort;
      setConnectionModelSelection(profileId, nextModel, nextEffort);
      setConnectionBusy(profileId, response.thread.status?.type === "active");
      upsertThread(profileId, response.thread);
    } catch (error) {
      if (threadOperationGateRef.current.isCurrent(
        token,
        activeConnectionIdRef.current,
        activeThreadIdRef.current,
      )) setToast(errorMessage(error));
    }
  }

  function newThread() {
    if (busy) return;
    const profileId = activeConnectionIdRef.current;
    threadOperationGateRef.current.invalidate();
    if (profileId) setConnectionThread(profileId, null);
    else {
      activeThreadIdRef.current = null;
      setActiveThread(null);
    }
    if (profileId) setConnectionPrompt(profileId, "");
    else setPrompt("");
    closeSidebarOnNarrowScreen();
  }

  function closeSidebarOnNarrowScreen() {
    if (viewport.compact) setSidebarOpen(false);
  }

  function openConnectionPanel() {
    setUsageOpen(false);
    if (viewport.compact) setSidebarOpen(false);
    if (connections.length) setConnectionManagerOpen(true);
    else setConnectOpen(true);
  }

  function openAddConnection() {
    setConnectionManagerOpen(false);
    setConnectError("");
    setPairingProgress("");
    setDeviceAuthPrompt(null);
    setConnectionMode(runtimeCapabilities.pairingSupported ? "manual" : "advanced");
    setConnectOpen(true);
  }

  function handleConversationScroll() {
    const conversation = conversationRef.current;
    if (!conversation) return;
    const nearBottom = isNearScrollBottom(
      conversation.scrollTop,
      conversation.scrollHeight,
      conversation.clientHeight,
    );
    stickToBottomRef.current = nearBottom;
    if (nearBottom) setHasNewMessages(false);
  }

  function scrollToLatest() {
    stickToBottomRef.current = true;
    setHasNewMessages(false);
    messageEndRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }

  async function sendPrompt() {
    const text = prompt.trim();
    const client = clientRef.current;
    const profileId = activeConnectionIdRef.current;
    if (!text || !client || !profileId || transportState !== "connected" || busy) return;
    setConnectionPrompt(profileId, "");
    setConnectionBusy(profileId, true);
    try {
      let threadId = activeThreadIdRef.current ?? undefined;
      if (!threadId) {
        const response = await client.request<{ thread: CodexThread }>("thread/start", {
          ...(selectedModel ? { model: selectedModel } : {}),
          ...(cwd.trim() ? { cwd: cwd.trim() } : {}),
          approvalPolicy: "on-request",
        });
        threadId = response.thread.id;
        if (isActiveClient(profileId, client)) {
          setConnectionThread(profileId, response.thread, response.thread.cwd || cwd.trim());
          upsertThread(profileId, response.thread);
        } else {
          const view = getConnectionView(profileId);
          connectionViewsRef.current.set(profileId, { ...view, activeThread: response.thread });
        }
      }

      const turnResponse = await client.request<{ turn: Turn }>("turn/start", {
        threadId,
        input: [{ type: "text", text, text_elements: [] }],
        ...(selectedModel ? { model: selectedModel } : {}),
        ...(selectedEffort ? { effort: selectedEffort } : {}),
      });
      updateConnectionThread(profileId, threadId, (thread) => ({
        ...thread,
        turns: replaceTurn(thread.turns, turnResponse.turn),
      }));
    } catch (error) {
      if (clientsRef.current.get(profileId) === client) {
        setConnectionBusy(profileId, false);
        restoreConnectionPromptIfEmpty(profileId, text);
      }
      if (isActiveClient(profileId, client)) {
        setToast(errorMessage(error));
      }
    }
  }

  async function interruptTurn() {
    const client = clientRef.current;
    const profileId = activeConnectionIdRef.current;
    if (!client || !profileId) return;
    if (!activeThread) {
      setToast("中断できる実行中のターンが見つかりません");
      return;
    }
    const turn = [...(activeThread.turns ?? [])]
      .reverse()
      .find((candidate) => candidate.status === "inProgress");
    if (!turn) {
      setToast("中断できる実行中のターンが見つかりません");
      return;
    }
    try {
      await client.request("turn/interrupt", turnInterruptParams(activeThread.id, turn.id));
    } catch (error) {
      if (isActiveClient(profileId, client)) setToast(errorMessage(error));
    }
  }

  async function archiveThread() {
    const thread = activeThread;
    const client = clientRef.current;
    const profileId = activeConnectionIdRef.current;
    if (!thread || !client || !profileId || busy) return;
    try {
      await client.request("thread/archive", { threadId: thread.id });
      if (!isActiveClient(profileId, client) || activeThreadIdRef.current !== thread.id) return;
      setConnectionThreads(profileId, (current) => current.filter((item) => item.id !== thread.id));
      threadOperationGateRef.current.invalidate();
      setConnectionThread(profileId, null);
      setToast("タスクをアーカイブしました");
    } catch (error) {
      if (isActiveClient(profileId, client)) setToast(errorMessage(error));
    }
  }

  async function resolveServerRequest(profileId: string, request: ServerRequest, result: unknown) {
    const client = clientsRef.current.get(profileId);
    if (!client) {
      if (activeConnectionIdRef.current === profileId) setToast("要求を受信した接続は利用できません");
      return;
    }
    try {
      await client.respond(request.id, result);
      setPendingRequestsForConnection(profileId, (current) => removePendingRequest(current, request.id));
    } catch (error) {
      if (activeConnectionIdRef.current === profileId) setToast(errorMessage(error));
    }
  }

  async function rejectUnsupportedServerRequest(profileId: string, request: ServerRequest) {
    const client = clientsRef.current.get(profileId);
    if (!client) {
      if (activeConnectionIdRef.current === profileId) setToast("要求を受信した接続は利用できません");
      return;
    }
    try {
      await client.respondError(request.id, -32601, `${request.method} はこのクライアントでは未対応です`);
      setPendingRequestsForConnection(profileId, (current) => removePendingRequest(current, request.id));
    } catch (error) {
      if (activeConnectionIdRef.current === profileId) setToast(errorMessage(error));
    }
  }

  function handleConnectionStatus(profileId: string, status: ConnectionStatus) {
    setConnections((current) => current.map((connection) => connection.id === profileId
      ? { ...connection, state: status.state, detail: status.detail }
      : connection));
    if (activeConnectionIdRef.current !== profileId || status.state === "connected") return;
    setTransportState("disconnected");
    setConnectionBusy(profileId, false);
    const view = getConnectionView(profileId);
    connectionViewsRef.current.set(profileId, {
      ...view,
      rateLimits: null,
      tokenUsage: null,
      usageLoading: false,
      usageError: "",
    });
    setUsageOpen(false);
    setRateLimits(null);
    setTokenUsage(null);
    setUsageError("");
    if (status.detail && status.detail !== "切断しました") setToast(status.detail);
    setConnectionManagerOpen(true);
  }

  function handleNotification(profileId: string, message: WireMessage) {
    const params = message.params as Record<string, any> | undefined;
    if (!params || !message.method) return;
    const isActiveConnection = activeConnectionIdRef.current === profileId;
    const storedThreadId = getConnectionView(profileId).activeThread?.id ?? null;
    switch (message.method) {
      case "thread/started":
        if (params.thread) upsertThread(profileId, params.thread as CodexThread);
        break;
      case "thread/status/changed":
        setConnectionThreads(profileId, (current) => current.map((thread) => (
          thread.id === params.threadId ? { ...thread, status: params.status } : thread
        )));
        updateConnectionThread(profileId, params.threadId, (thread) => ({ ...thread, status: params.status }));
        break;
      case "thread/name/updated":
        setConnectionThreads(profileId, (current) => current.map((thread) => (
          thread.id === params.threadId ? { ...thread, name: params.name } : thread
        )));
        updateConnectionThread(profileId, params.threadId, (thread) => ({ ...thread, name: params.name }));
        break;
      case "turn/started":
        if (shouldApplyThreadBusy(
          activeConnectionIdRef.current,
          profileId,
          activeThreadIdRef.current,
          params.threadId,
        ) || (!isActiveConnection && storedThreadId === params.threadId)) {
          setConnectionBusy(profileId, true);
        }
        updateConnectionThread(profileId, params.threadId, (thread) => ({
          ...thread,
          turns: replaceTurn(thread.turns, params.turn),
        }));
        break;
      case "item/started":
      case "item/completed":
        updateConnectionThread(profileId, params.threadId, (thread) => ({
          ...thread,
          turns: updateTurnItem(thread.turns, params.turnId, params.item),
        }));
        break;
      case "item/agentMessage/delta":
        updateConnectionThread(profileId, params.threadId, (thread) => ({
          ...thread,
          turns: appendAgentDelta(thread.turns, params.turnId, params.itemId, params.delta),
        }));
        break;
      case "item/commandExecution/outputDelta":
        updateConnectionThread(profileId, params.threadId, (thread) => ({
          ...thread,
          turns: appendOutputDelta(thread.turns, params.turnId, params.itemId, params.delta),
        }));
        break;
      case "turn/completed":
        if (shouldApplyThreadBusy(
          activeConnectionIdRef.current,
          profileId,
          activeThreadIdRef.current,
          params.threadId,
        ) || (!isActiveConnection && storedThreadId === params.threadId)) {
          setConnectionBusy(profileId, false);
        }
        updateConnectionThread(profileId, params.threadId, (thread) => ({
          ...thread,
          updatedAt: Math.floor(Date.now() / 1000),
          turns: mergeCompletedTurn(thread.turns, params.turn),
        }));
        if (isActiveConnection) {
          void refreshCatalogs();
          void refreshUsage();
        }
        break;
      case "account/rateLimits/updated":
        if (isActiveConnection) void refreshUsage();
        break;
      case "serverRequest/resolved":
        setPendingRequestsForConnection(profileId, (current) => removePendingRequest(current, params.requestId));
        break;
      case "thread/archived":
      case "thread/deleted":
        setConnectionThreads(profileId, (current) => current.filter((thread) => thread.id !== params.threadId));
        if (storedThreadId === params.threadId) {
          if (isActiveConnection) threadOperationGateRef.current.invalidate();
          setConnectionBusy(profileId, false);
          setConnectionThread(profileId, null);
        }
        break;
      case "thread/closed":
        if (storedThreadId === params.threadId) {
          if (isActiveConnection) threadOperationGateRef.current.invalidate();
          setConnectionBusy(profileId, false);
          setConnectionThread(profileId, null);
        }
        break;
      case "error":
      case "warning":
      case "configWarning":
        if (isActiveConnection) setToast(params.message || params.error?.message || message.method);
        break;
    }
  }

  function upsertThread(profileId: string, thread: CodexThread) {
    setConnectionThreads(profileId, (current) => {
      const rest = current.filter((item) => item.id !== thread.id);
      return [thread, ...rest].sort(
        (a, b) => (b.recencyAt ?? b.updatedAt) - (a.recencyAt ?? a.updatedAt),
      );
    });
  }

  function handleComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (shouldSubmitComposer({
      key: event.key,
      shiftKey: event.shiftKey,
      isComposing: event.nativeEvent.isComposing,
      keyCode: event.nativeEvent.keyCode,
    })) {
      event.preventDefault();
      void sendPrompt();
    }
  }

  async function runWindowAction(action: "minimize" | "toggleMaximize" | "close") {
    try {
      const appWindow = getCurrentWindow();
      if (action === "minimize") await appWindow.minimize();
      else if (action === "toggleMaximize") {
        const maximized = await appWindow.isMaximized();
        if (maximized) await appWindow.unmaximize();
        else await appWindow.maximize();
      }
      else await appWindow.close();
    } catch {
      // Browser-based design preview has no native window.
    }
  }

  function showDesktopOnlyFeature(label: string) {
    setToast(`${label}は接続先のCodex Desktopで利用できます`);
  }

  return (
    <div className={`desktop-frame ${viewport.keyboardOpen ? "keyboard-visible" : ""}`}>
      <div className="application-titlebar" aria-hidden={runtimeCapabilities.mobile || undefined}>
        <div className="titlebar-navigation">
          <button className="titlebar-icon-button" onClick={() => setSidebarOpen((current) => !current)} title="サイドバーを切り替える" aria-label="サイドバーを切り替える"><PanelLeft size={14} /></button>
          <button className="titlebar-icon-button" disabled title="戻る"><ArrowLeft size={14} /></button>
          <button className="titlebar-icon-button" disabled title="進む"><ArrowRight size={14} /></button>
        </div>
        <nav className="application-menu" aria-label="アプリケーションメニュー">
          {['ファイル', '編集', '表示', 'ヘルプ'].map((label) => <button key={label} onClick={() => showDesktopOnlyFeature(label)}>{label}</button>)}
        </nav>
        <div className="titlebar-drag-space" data-tauri-drag-region onDoubleClick={() => void runWindowAction("toggleMaximize")} />
        <div className="window-controls">
          <button onClick={() => void runWindowAction("minimize")} title="最小化"><Minus size={14} /></button>
          <button onClick={() => void runWindowAction("toggleMaximize")} title="最大化"><Square size={10} /></button>
          <button className="window-close-button" onClick={() => void runWindowAction("close")} title="閉じる"><X size={14} /></button>
        </div>
      </div>

      <div className={`app-shell ${sidebarOpen ? "sidebar-visible" : ""} ${connectOpen ? "connect-dialog-open" : ""}`}>
      <aside
        ref={sidebarRef}
        className={`sidebar ${sidebarOpen ? "is-open" : ""}`}
        role={viewport.compact ? "dialog" : undefined}
        aria-modal={viewport.compact && sidebarOpen ? true : undefined}
        aria-label={viewport.compact ? "タスク一覧" : undefined}
        aria-hidden={viewport.compact && !sidebarOpen ? true : undefined}
        tabIndex={viewport.compact ? -1 : undefined}
        inert={overlayOpen ? true : undefined}
      >
        <div className="sidebar-toolbar">
          <div className="codex-wordmark">Codex <ChevronDown size={13} /></div>
          <button className="icon-button" onClick={() => setSearchOpen((current) => !current)} title="タスクを検索">
            <Search size={17} />
          </button>
          <button className="icon-button" onClick={newThread} disabled={busy} title="新しいタスク">
            <SquarePen size={18} />
          </button>
        </div>

        <div className="sidebar-actions">
          <button className="new-task-button" onClick={newThread} disabled={busy}>
            <SquarePen size={16} /> 新しいタスク
          </button>
          <button className="new-task-button" onClick={() => showDesktopOnlyFeature("プルリクエスト")}><GitPullRequest size={16} /> プルリクエスト</button>
          <button className="new-task-button" onClick={() => showDesktopOnlyFeature("サイト")}><Globe2 size={16} /> サイト</button>
          <button className="new-task-button" onClick={() => showDesktopOnlyFeature("スケジュール")}><Clock3 size={16} /> スケジュール</button>
          <button className="new-task-button" onClick={() => showDesktopOnlyFeature("プラグイン")}><Blocks size={16} /> プラグイン</button>
          {searchOpen && (
            <div className="thread-search">
              <Search size={15} />
              <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="タスクを検索" autoFocus />
              {search && <button onClick={() => setSearch("")} title="検索を消去"><X size={13} /></button>}
            </div>
          )}
        </div>

        <nav className="thread-list">
          <div className="project-section-label">
            <span>プロジェクト</span>
            <button className="icon-button" onClick={() => void refreshCatalogs()} title="更新">
              <RefreshCw size={13} />
            </button>
          </div>
          {threadGroups.map((group) => (
            <section className="thread-group" key={group.label}>
              <div className="thread-section-label">
                <Folder size={14} />
                <span>{group.label}</span>
              </div>
              {group.threads.map((thread) => (
                <button
                  key={thread.id}
                  className={`thread-row ${activeThread?.id === thread.id ? "is-active" : ""}`}
                  onClick={() => void openThread(thread)}
                >
                  <span className="thread-copy">
                    <span className="thread-title">{thread.name || thread.preview || "無題のタスク"}</span>
                  </span>
                  <span className={`thread-status status-${thread.status?.type || "idle"}`} />
                </button>
              ))}
            </section>
          ))}
          {visibleThreads.length === 0 && (
            <div className="empty-list">{catalogLoading ? "タスクを読み込み中" : "タスクはまだありません"}</div>
          )}
        </nav>

        <div className="sidebar-footer">
          <button
            className="usage-chip"
            onClick={() => {
              setConnectOpen(false);
              if (viewport.compact) setSidebarOpen(false);
              setUsageOpen(true);
              if (!rateLimits && transportState === "connected") void refreshUsage();
            }}
            title="使用状況"
          >
            <span className="usage-chip-icon"><ChartNoAxesColumnIncreasing size={15} /></span>
            <span className="usage-chip-copy">
              <strong>使用状況</strong>
              <small>
                {compactUsage.length
                  ? compactUsage.map((row) => `${row.compactLabel} ${Math.round(row.remainingPercent)}%`).join(" · ")
                  : transportState === "connected" ? "利用制限を読み込む" : "接続後に表示"}
              </small>
            </span>
            <ChevronRight size={14} />
          </button>

          <button className="connection-chip" onClick={openConnectionPanel} title="接続先を管理">
            <span className="connection-icon">
              {transportState === "connected" ? <Wifi size={15} /> : <WifiOff size={15} />}
            </span>
            <span className="connection-copy">
              <strong>{transportState === "connected" ? connectionLabel || "Remote" : "App Server に接続"}</strong>
              <small>{transportState === "connected" ? `${serverInfo?.platformOs || "remote"} · ${connections.length}台接続` : "オフライン"}</small>
            </span>
            {connections.length > 1 && <span className="connection-count">{connections.length}</span>}
            <ChevronDown size={14} />
          </button>
        </div>
      </aside>

      {sidebarOpen && viewport.compact && <button className="sidebar-scrim" onClick={() => setSidebarOpen(false)} aria-label="サイドバーを閉じる" />}

      <main className="main-panel" inert={(overlayOpen || (sidebarOpen && viewport.compact)) ? true : undefined}>
        <header className="topbar">
          <button className={`icon-button sidebar-open-button ${sidebarOpen ? "is-hidden" : ""}`} onClick={() => setSidebarOpen(true)} title="サイドバーを開く"><PanelLeft size={18} /></button>
          <button className={`icon-button quick-new-button ${sidebarOpen ? "is-hidden" : ""}`} onClick={newThread} disabled={busy} title="新しいタスク"><SquarePen size={18} /></button>
          <div className="task-heading">
            <Folder size={16} />
            <h1>{activeThread?.name || activeThread?.preview || "新しいタスク"}</h1>
          </div>
          <div className="topbar-actions">
            {activeThread && (
              <button className="icon-button" onClick={() => void archiveThread()} disabled={busy} title="アーカイブ">
                <Archive size={17} />
              </button>
            )}
            <button className={`server-button ${transportState}`} onClick={openConnectionPanel}>
              <span className="server-state-dot" />
              <span>{transportState === "connected" ? connections.length > 1 ? `${connections.length}台` : "Remote" : "接続"}</span>
            </button>
            <button className="icon-button" onClick={openConnectionPanel} title="接続先"><MoreHorizontal size={18} /></button>
          </div>
        </header>

        <section
          className="conversation"
          ref={conversationRef}
          onScroll={handleConversationScroll}
          aria-live="polite"
          aria-busy={busy}
        >
          {!activeThread?.turns?.length ? (
            <EmptyState connected={transportState === "connected"} onConnect={openConnectionPanel} />
          ) : (
            <div className="message-stack">
              {activeThread.turns.flatMap((turn) =>
                turn.items.map((item, index) => (
                  <MessageItem key={`${turn.id}-${item.id || index}`} item={item} />
                )),
              )}
              {busy && <WorkingIndicator />}
            </div>
          )}

          {serverRequests.map((request) => (
            <RequestCard
              key={requestCardKey(activeConnectionId, request.id)}
              request={request}
              onResolve={(result) => {
                if (activeConnectionId) void resolveServerRequest(activeConnectionId, request, result);
              }}
              onUnsupported={() => {
                if (activeConnectionId) void rejectUnsupportedServerRequest(activeConnectionId, request);
              }}
            />
          ))}
          <div ref={messageEndRef} />
        </section>

        {hasNewMessages && (
          <button type="button" className="conversation-latest-button" onClick={scrollToLatest}>
            <ArrowUp size={14} /> 最新へ移動
          </button>
        )}

        <section className="composer-wrap">
          <div className={`composer ${busy ? "is-busy" : ""}`}>
            <textarea
              ref={composerTextareaRef}
              value={prompt}
              onChange={(event) => updatePrompt(event.target.value)}
              onKeyDown={handleComposerKeyDown}
              placeholder={transportState === "connected" ? "何でもどうぞ" : "先に App Server へ接続してください"}
              disabled={transportState !== "connected"}
              aria-label="Codexへのメッセージ"
              rows={1}
            />
            <div className="composer-toolbar">
              <div className="composer-tools">
                <button className="composer-icon-button" disabled title="添付ファイル（準備中）"><Plus size={18} /></button>
                {activeThread && <button className="access-mode-button" onClick={() => showDesktopOnlyFeature("フルアクセス設定")} title="アクセスモード"><Shield size={13} /> フルアクセス</button>}
                {!activeThread && (
                  <label className="composer-path">
                    <Folder size={13} />
                    <input value={cwd} onChange={(event) => {
                      const profileId = activeConnectionIdRef.current;
                      if (profileId) setConnectionCwd(profileId, event.target.value);
                      else setCwd(event.target.value);
                    }} placeholder="作業フォルダー" disabled={busy} aria-label="作業フォルダー" />
                  </label>
                )}
              </div>
              <div className="composer-actions">
                <div className={`model-picker ${modelPickerOpen ? "is-open" : ""}`} ref={modelPickerRef}>
                  <button
                    type="button"
                    className="model-picker-trigger"
                    onClick={() => setModelPickerOpen((current) => !current)}
                    disabled={transportState !== "connected" || busy || !models.length}
                    aria-label="モデルを選択"
                    aria-haspopup="menu"
                    aria-expanded={modelPickerOpen}
                  >
                    <span className="model-picker-trigger-label">
                      <span className="model-picker-model-name">{compactModelName(activeModel?.displayName || selectedModel) || "モデルを選択"}</span>
                      {selectedEffort && <span className="model-picker-effort-name">{effortLabel(selectedEffort)}</span>}
                    </span>
                    <ChevronDown size={14} />
                  </button>

                  {modelPickerOpen && (
                    <div className="model-picker-menu" role="menu" aria-label="モデルと推論レベル">
                      <div className="model-picker-section-title">モデル</div>
                      <div className="model-picker-list">
                        {models.map((model) => (
                          <button
                            type="button"
                            className={`model-picker-item ${model.model === selectedModel ? "is-selected" : ""}`}
                            key={model.id}
                            role="menuitemradio"
                            aria-checked={model.model === selectedModel}
                            onClick={() => selectComposerModel(model)}
                          >
                            <span className="model-picker-item-copy">
                              <strong>{compactModelName(model.displayName || model.model)}</strong>
                              {model.description && <small>{model.description}</small>}
                            </span>
                            {model.model === selectedModel && <Check size={15} />}
                          </button>
                        ))}
                      </div>

                      <div className="model-picker-divider" />
                      <div className="model-picker-section-title reasoning-title">
                        <span>推論レベル</span>
                        <strong>{effortLabel(selectedEffort)}</strong>
                      </div>
                      <div className="reasoning-options" role="group" aria-label="推論レベル">
                        {efforts.map((effort) => (
                          <button
                            type="button"
                            className={effort.reasoningEffort === selectedEffort ? "is-selected" : ""}
                            key={effort.reasoningEffort}
                            onClick={() => {
                              const profileId = activeConnectionIdRef.current;
                              if (profileId) {
                                setConnectionModelSelection(
                                  profileId,
                                  selectedModel,
                                  effort.reasoningEffort,
                                );
                              } else {
                                setSelectedEffort(effort.reasoningEffort);
                              }
                              setModelPickerOpen(false);
                            }}
                            aria-pressed={effort.reasoningEffort === selectedEffort}
                            title={effort.description}
                          >
                            <span className="reasoning-dot" />
                            <span>{effortLabel(effort.reasoningEffort)}</span>
                          </button>
                        ))}
                      </div>
                      <div className="reasoning-scale" aria-hidden="true"><span>高速</span><span>より賢く</span></div>
                    </div>
                  )}
                </div>

                {busy ? (
                  <button className="send-button stop" onClick={() => void interruptTurn()} title="中断"><CircleStop size={18} /></button>
                ) : (
                  <>
                    <button className="composer-icon-button microphone-button" onClick={() => showDesktopOnlyFeature("音声入力")} disabled={transportState !== "connected"} title="音声入力"><Mic size={16} /></button>
                    <button className="send-button" onClick={() => void sendPrompt()} disabled={!prompt.trim() || transportState !== "connected"} title="送信"><ArrowUp size={18} /></button>
                  </>
                )}
              </div>
            </div>
          </div>
        </section>
      </main>

      <nav className="mobile-bottom-nav" aria-label="モバイルナビゲーション" inert={overlayOpen ? true : undefined}>
        <button type="button" className={sidebarOpen ? "is-active" : ""} aria-current={sidebarOpen ? "page" : undefined} aria-expanded={sidebarOpen} onClick={() => setSidebarOpen(true)}><PanelLeft size={19} /><span>タスク</span></button>
        <button type="button" className="mobile-new-task" onClick={newThread} disabled={busy}><SquarePen size={19} /><span>新規</span></button>
        <button type="button" className={connectionManagerOpen ? "is-active" : ""} aria-current={connectionManagerOpen ? "page" : undefined} onClick={openConnectionPanel}>
          <span className="mobile-nav-icon"><Server size={19} />{connections.length > 1 && <i>{connections.length}</i>}</span>
          <span>接続先</span>
        </button>
        <button type="button" className={usageOpen ? "is-active" : ""} aria-current={usageOpen ? "page" : undefined} onClick={() => {
          setSidebarOpen(false);
          setConnectionManagerOpen(false);
          setUsageOpen(true);
          if (!rateLimits && transportState === "connected") void refreshUsage();
        }}><ChartNoAxesColumnIncreasing size={19} /><span>使用状況</span></button>
      </nav>

      <ConnectionSwitcher
        open={connectionManagerOpen}
        connections={connections}
        activeId={activeConnectionId}
        onClose={() => setConnectionManagerOpen(false)}
        onSelect={(id) => void switchConnection(id)}
        onAdd={openAddConnection}
        onDisconnect={(id) => void disconnectConnection(id)}
      />

      {connectOpen && (
        <div className="modal-backdrop" onPointerDown={(event) => event.target === event.currentTarget && closeConnectDialog()}>
          <form ref={connectDialogRef} tabIndex={-1} className="connect-modal pair-modal" role="dialog" aria-modal="true" aria-labelledby="connect-dialog-title" onSubmit={submitConnection}>
            <button type="button" className="icon-button modal-close" onClick={closeConnectDialog} disabled={connectDialogLocked} aria-label="接続画面を閉じる"><X size={18} /></button>
            <div className="modal-heading">
              <h2 id="connect-dialog-title">接続を追加</h2>
              <p>接続先のCodexに表示されたPairコード、またはQRコードを使用します。</p>
            </div>
            <div className="connection-tabs" role="tablist" aria-label="接続方法">
              <button type="button" disabled={connectDialogLocked || !runtimeCapabilities.pairingSupported} className={connectionMode === "manual" ? "active" : ""} onClick={() => changeConnectionMode("manual")}><Keyboard size={15} /> Pairコード</button>
              <button type="button" disabled={connectDialogLocked || !runtimeCapabilities.pairingSupported} className={connectionMode === "qr" ? "active" : ""} onClick={() => changeConnectionMode("qr")}><QrCode size={15} /> QR Pair</button>
              <button type="button" disabled={connectDialogLocked} className={connectionMode === "advanced" ? "active" : ""} onClick={() => changeConnectionMode("advanced")}><Code2 size={15} /> 上級者向け</button>
            </div>

            {!runtimeCapabilities.pairingSupported && (
              <div className="pair-unavailable" role="status">
                <ShieldAlert size={16} />
                <span>この端末では公式Pair用のOS保護端末鍵を利用できないため、上級者向け接続のみ使用できます。</span>
              </div>
            )}

            {connectionMode === "manual" && (
              <div className="pair-panel">
                {!pairPrepared ? (
                  <button type="button" className="secondary-button" disabled={connectionAdding} onClick={() => void preparePairAuthentication()}>
                    <Shield size={15} /> Codex認証を準備
                  </button>
                ) : (
                  <div className="pair-unavailable" role="status"><Check size={16} /><span>準備完了。接続先で新しいPairコードを発行してください。</span></div>
                )}
                <label className="form-field pair-code-field">
                  <span>Pairコード</span>
                  <div className="input-with-icon"><Keyboard size={16} /><input value={pairCode} disabled={!pairPrepared} onChange={(event) => setPairCode(event.target.value)} placeholder="接続先に表示されたコード" autoFocus autoComplete="one-time-code" autoCapitalize="characters" spellCheck={false} inputMode="text" /></div>
                </label>
                <p className="pair-help">接続先のCodexで「Remote」→「Connect a device」を開き、表示されたコードを入力してください。</p>
              </div>
            )}

            {connectionMode === "qr" && (
              <div className="pair-panel qr-panel">
                {!pairPrepared ? (
                  <button type="button" className="secondary-button" disabled={connectionAdding} onClick={() => void preparePairAuthentication()}>
                    <Shield size={15} /> Codex認証を準備
                  </button>
                ) : (
                  <div className="pair-unavailable" role="status"><Check size={16} /><span>準備完了。接続先で新しいQR Pairを発行してください。</span></div>
                )}
                <div className={`qr-preview ${qrScanning ? "scanning" : ""}`}>
                  <video ref={qrVideoRef} muted playsInline aria-label="QRコードのカメラプレビュー" />
                  {!qrScanning && <div className="qr-placeholder"><QrCode size={40} /><span>接続先のQRコードを読み取る</span></div>}
                  {qrScanning && <span className="scan-line" />}
                </div>
                <div className="qr-actions">
                  <button type="button" className="secondary-button" disabled={qrStarting || !pairPrepared} onClick={() => qrScanning ? stopQrCamera() : void startQrCamera()}><Camera size={15} /> {qrStarting ? "カメラを起動中" : qrScanning ? "カメラを停止" : "カメラで読取"}</button>
                  <label className="secondary-button file-button"><ImagePlus size={15} /> 画像を選択<input type="file" accept="image/*" disabled={qrStarting || !pairPrepared} onChange={(event) => void readQrImage(event.target.files?.[0])} /></label>
                </div>
                <label className="form-field compact-field">
                  <span>またはQRのURLを貼り付け</span>
                  <div className="input-with-icon"><QrCode size={16} /><input value={qrValue} disabled={!pairPrepared} onChange={(event) => setQrValue(event.target.value)} placeholder="https://chatgpt.com/codex/pair?pairing_code=…" /></div>
                </label>
              </div>
            )}

            {connectionMode === "advanced" && (
              <div className="pair-panel advanced-panel">
                <div className="advanced-badge"><ShieldAlert size={14} /> 上級者向け直接接続</div>
                <label className="form-field">
                  <span>WebSocket URL</span>
                  <div className="input-with-icon"><Code2 size={16} /><input value={endpoint} onChange={(event) => setEndpoint(event.target.value)} placeholder="wss://codex.example.com:4500" autoFocus /></div>
                </label>
                <label className="form-field">
                  <span>Bearer Token <em>任意</em></span>
                  <div className="input-with-icon"><ShieldAlert size={16} /><input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder="接続時だけ使用し、保存しません" autoComplete="off" /></div>
                </label>
                <div className="security-note"><ShieldAlert size={16} /><span>リモート接続には <strong>wss://</strong> を使用してください。<strong>ws://</strong> はlocalhost / SSHポートフォワード専用です。</span></div>
              </div>
            )}
            {connectionAdding && deviceAuthPrompt?.verificationUrl && deviceAuthPrompt.userCode && (
              <div className="device-auth-card" role="status" aria-live="polite">
                <div className="device-auth-title"><Shield size={16} /><strong>Codex CLI デバイスコード認証</strong></div>
                <p>ブラウザで次のコードを入力してください。自分で開始したこのログインにだけ使用してください。</p>
                <output className="device-auth-code" aria-label="ワンタイムデバイスコード">{deviceAuthPrompt.userCode}</output>
                <div className="device-auth-url">{deviceAuthPrompt.verificationUrl}</div>
                <button type="button" className="secondary-button" onClick={() => void navigator.clipboard.writeText(deviceAuthPrompt.userCode || "").then(() => setToast("デバイスコードをコピーしました")).catch(() => setToast("デバイスコードをコピーできませんでした"))}>コードをコピー</button>
              </div>
            )}
            {connectionAdding && pairingProgress && <div className="pair-progress"><LoaderCircle className="spin" size={16} /><span>{pairingProgress}</span></div>}
            {connectError && <div className="connect-error">{connectError}</div>}
            <div className="modal-actions">
              <button
                type="button"
                className="secondary-button"
                onClick={() => connectionAdding ? void cancelAddingConnection() : closeConnectDialog()}
                disabled={qrStarting || connectionCancelling}
              >{connectionCancelling ? "キャンセル中" : "キャンセル"}</button>
              <button className="primary-button" disabled={connectDialogLocked || !transportReady || (connectionMode === "manual" ? !pairPrepared || !pairCode.trim() : connectionMode === "qr" ? !pairPrepared || !qrValue.trim() : !endpoint.trim())}>
                {connectionAdding ? <><LoaderCircle className="spin" size={16} /> 接続中</> : <>{connectionMode === "advanced" ? <Server size={16} /> : <QrCode size={16} />} {connectionMode === "advanced" ? "直接接続" : "Pairして接続"}</>}
              </button>
            </div>
          </form>
        </div>
      )}

      <UsageSettings
        open={usageOpen}
        connected={transportState === "connected"}
        loading={usageLoading}
        error={usageError}
        rateLimits={rateLimits}
        tokenUsage={tokenUsage}
        onClose={() => setUsageOpen(false)}
        onRefresh={() => void refreshUsage()}
      />

      {toast && <div className="toast"><ShieldAlert size={16} /><span>{toast}</span><button onClick={() => setToast("")}><X size={14} /></button></div>}
      </div>
    </div>
  );
}

function EmptyState({ connected, onConnect }: { connected: boolean; onConnect: () => void }) {
  return (
    <div className="empty-state">
      <h2>{connected ? "今日は何をしますか？" : "Codex に接続"}</h2>
      <p>{connected ? "リモートのコードベースにタスクを依頼できます。" : "Codex App Server に接続すると、ここからタスクを開始できます。"}</p>
      {!connected && <button className="primary-button" onClick={onConnect}><Server size={16} /> 接続設定を開く</button>}
    </div>
  );
}

function MessageItem({ item }: { item: ThreadItem }) {
  if (item.type === "userMessage") {
    const text = Array.isArray(item.content)
      ? item.content.filter((input) => typeof input === "object" && input?.type === "text").map((input) => "text" in input ? input.text : "").join("\n")
      : String(item.content || "");
    return <div className="message user-message"><div className="user-bubble">{text}</div></div>;
  }

  if (item.type === "agentMessage") {
    return (
      <article className="message agent-message">
        <div className="markdown">
          <ReactMarkdown
            remarkPlugins={[remarkGfm]}
            components={{
              table: ({ children }) => (
                <div className="markdown-table-scroll" tabIndex={0} aria-label="表を横スクロール">
                  <table>{children}</table>
                </div>
              ),
            }}
          >
            {item.text || ""}
          </ReactMarkdown>
        </div>
      </article>
    );
  }

  if (item.type === "reasoning") {
    const summary = Array.isArray(item.summary) ? item.summary.join("\n") : "推論";
    return <details className="activity-card reasoning-card"><summary><Sparkles size={15} /> 推論 <ChevronDown size={14} /></summary><div>{summary}</div></details>;
  }

  if (item.type === "commandExecution") {
    return (
      <details className="activity-card" open={item.status === "inProgress"}>
        <summary><Terminal size={15} /><span className="activity-title">コマンド</span><code>{item.command}</code><StatusPill status={item.status} /><ChevronDown size={14} /></summary>
        <div className="activity-detail"><div className="activity-path">{item.cwd}</div>{item.aggregatedOutput && <pre>{item.aggregatedOutput}</pre>}</div>
      </details>
    );
  }

  if (item.type === "fileChange") {
    return (
      <details className="activity-card">
        <summary><FileCode2 size={15} /><span className="activity-title">ファイル変更</span><span>{Array.isArray(item.changes) ? `${item.changes.length} 件` : ""}</span><StatusPill status={item.status} /><ChevronDown size={14} /></summary>
        <div className="activity-detail change-list">{(item.changes || []).map((change, index) => <code key={index}>{String(change.path ?? change.file ?? JSON.stringify(change))}</code>)}</div>
      </details>
    );
  }

  if (item.type === "mcpToolCall" || item.type === "dynamicToolCall") {
    return (
      <details className="activity-card">
        <summary><Wrench size={15} /><span className="activity-title">{item.server ? `${item.server} / ` : ""}{String(item.tool || "ツール")}</span><StatusPill status={item.status} /><ChevronDown size={14} /></summary>
        <div className="activity-detail"><pre>{JSON.stringify(item.arguments ?? item.result ?? item.error, null, 2)}</pre></div>
      </details>
    );
  }

  if (item.type === "plan") {
    return <div className="plan-card"><Check size={15} /><div><strong>プラン</strong><p>{item.text}</p></div></div>;
  }

  return null;
}

function StatusPill({ status }: { status: unknown }) {
  const value = String(status || "");
  return <span className={`status-pill ${value.toLowerCase()}`}>{value === "inProgress" ? "実行中" : value || "待機中"}</span>;
}

function WorkingIndicator() {
  return <div className="working"><LoaderCircle className="spin" size={14} /><span>作業中</span><span className="working-dots">•••</span></div>;
}

function RequestCard({
  request,
  onResolve,
  onUnsupported,
}: {
  request: ServerRequest;
  onResolve: (result: unknown) => void;
  onUnsupported: () => void;
}) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [validationError, setValidationError] = useState("");
  const params = request.params as Record<string, any>;
  const isCommand = request.method === "item/commandExecution/requestApproval" || request.method === "execCommandApproval";
  const isFile = request.method === "item/fileChange/requestApproval" || request.method === "applyPatchApproval";
  const isPermission = request.method === "item/permissions/requestApproval";
  const isQuestion = request.method === "item/tool/requestUserInput";
  const isMcp = request.method === "mcpServer/elicitation/request";
  const isDynamicTool = request.method === "item/tool/call";
  const isUnsupported = request.method === "account/chatgptAuthTokens/refresh"
    || request.method === "attestation/generate"
    || (!isCommand && !isFile && !isPermission && !isQuestion && !isMcp && !isDynamicTool);

  const title = isCommand ? "コマンド実行の承認" : isFile ? "ファイル変更の承認" : isPermission ? "追加権限の承認" : isQuestion ? "Codex からの質問" : isMcp ? "MCP の確認" : isDynamicTool ? "動的ツール呼び出し" : "未対応のクライアント操作";
  const description = params.reason || params.message || params.command || request.method;
  const missingQuestionAnswer = isQuestion && (params.questions || [])
    .some((question: any) => !String(answers[question.id] || "").trim());
  const missingMcpAnswer = isMcp && params.mode !== "url" && (params.requestedSchema?.required || [])
    .some((key: string) => !String(answers[key] || "").trim());

  function allow(scope: "turn" | "session" = "turn") {
    if (request.method === "item/commandExecution/requestApproval") onResolve({ decision: scope === "session" ? "acceptForSession" : "accept" });
    else if (request.method === "item/fileChange/requestApproval") onResolve({ decision: scope === "session" ? "acceptForSession" : "accept" });
    else if (request.method === "execCommandApproval" || request.method === "applyPatchApproval") onResolve({ decision: scope === "session" ? "approved_for_session" : "approved" });
    else if (isPermission) onResolve({ permissions: compactPermissions(params.permissions), scope });
    else if (isMcp) {
      if (missingMcpAnswer) {
        setValidationError("必須項目を入力してください");
        return;
      }
      onResolve({ action: "accept", content: mcpFormContent(params, answers), _meta: params._meta ?? null });
    }
  }

  function deny() {
    if (request.method === "item/commandExecution/requestApproval" || request.method === "item/fileChange/requestApproval") onResolve({ decision: "decline" });
    else if (request.method === "execCommandApproval" || request.method === "applyPatchApproval") onResolve({ decision: "denied" });
    else if (isPermission) onResolve({ permissions: {}, scope: "turn" });
    else if (isMcp) onResolve({ action: "decline", content: null, _meta: params._meta ?? null });
    else if (isDynamicTool) onResolve({ contentItems: [{ type: "inputText", text: "Client declined unsupported tool call" }], success: false });
    else onUnsupported();
  }

  function submitAnswers() {
    if (missingQuestionAnswer) {
      setValidationError("すべての質問へ回答してください");
      return;
    }
    const mapped: Record<string, { answers: string[] }> = {};
    for (const question of params.questions || []) mapped[question.id] = { answers: [answers[question.id] || ""] };
    onResolve({ answers: mapped });
  }

  return (
    <div className="request-card">
      <div className="request-icon"><ShieldAlert size={18} /></div>
      <div className="request-content">
        <h3>{title}</h3>
        <p>{String(description)}</p>
        {isCommand && params.cwd && <code className="request-path">{params.cwd}</code>}
        {isPermission && <pre>{JSON.stringify(params.permissions, null, 2)}</pre>}
        {isQuestion && (params.questions || []).map((question: any) => (
          <label className="question-field" key={question.id}>
            <span>{question.header || question.question}</span>
            {question.options?.length ? (
              <select required aria-invalid={validationError && !answers[question.id] ? true : undefined} value={answers[question.id] || ""} onChange={(event) => { setValidationError(""); setAnswers((current) => ({ ...current, [question.id]: event.target.value })); }}>
                <option value="">選択してください</option>
                {question.options.map((option: any) => <option key={option.label} value={option.label}>{option.label}</option>)}
              </select>
            ) : (
              <input required aria-invalid={validationError && !answers[question.id] ? true : undefined} type={question.isSecret ? "password" : "text"} value={answers[question.id] || ""} onChange={(event) => { setValidationError(""); setAnswers((current) => ({ ...current, [question.id]: event.target.value })); }} />
            )}
          </label>
        ))}
        {isMcp && params.mode !== "url" && Object.entries(params.requestedSchema?.properties || {}).map(([key, schema]: [string, any]) => (
          <label className="question-field" key={key}>
            <span>{schema.title || key}{params.requestedSchema?.required?.includes(key) ? " *" : ""}</span>
            {schema.enum?.length ? (
              <select value={answers[key] || ""} onChange={(event) => setAnswers((current) => ({ ...current, [key]: event.target.value }))}>
                <option value="">選択してください</option>
                {schema.enum.map((value: unknown) => <option key={String(value)} value={String(value)}>{String(value)}</option>)}
              </select>
            ) : schema.type === "boolean" ? (
              <select value={answers[key] || ""} onChange={(event) => setAnswers((current) => ({ ...current, [key]: event.target.value }))}>
                <option value="">選択してください</option><option value="true">はい</option><option value="false">いいえ</option>
              </select>
            ) : (
              <input type={schema.type === "number" || schema.type === "integer" ? "number" : schema.format === "password" ? "password" : "text"} value={answers[key] || ""} onChange={(event) => setAnswers((current) => ({ ...current, [key]: event.target.value }))} />
            )}
          </label>
        ))}
        {isMcp && params.mode === "url" && <code className="request-path">{params.url}</code>}
        {validationError && <div className="request-validation-error" role="alert">{validationError}</div>}
        <div className="request-actions">
          {isQuestion ? (
            <button className="primary-button small" onClick={submitAnswers} disabled={missingQuestionAnswer}><Send size={14} /> 回答する</button>
          ) : (
            <>
              <button className="secondary-button small danger" onClick={deny}>{isUnsupported ? "未対応として返す" : "拒否"}</button>
              {(isCommand || isFile || isPermission) && <button className="secondary-button small" onClick={() => allow("session")}>セッション中許可</button>}
              {(isCommand || isFile || isPermission || isMcp) && <button className="primary-button small" onClick={() => allow("turn")} disabled={isMcp && missingMcpAnswer}><Check size={14} /> 許可</button>}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function compactPermissions(permissions: any) {
  const result: Record<string, unknown> = {};
  if (permissions?.network) result.network = permissions.network;
  if (permissions?.fileSystem) result.fileSystem = permissions.fileSystem;
  return result;
}

function mcpFormContent(params: any, values: Record<string, string>) {
  if (params.mode === "url") return {};
  const result: Record<string, unknown> = {};
  for (const [key, schema] of Object.entries(params.requestedSchema?.properties || {}) as Array<[string, any]>) {
    const value = values[key];
    if (value === undefined || value === "") continue;
    if (schema.type === "boolean") result[key] = value === "true";
    else if (schema.type === "number" || schema.type === "integer") result[key] = Number(value);
    else result[key] = value;
  }
  return result;
}

function replaceTurn(turns: Turn[] = [], next: Turn) {
  const index = turns.findIndex((turn) => turn.id === next.id);
  if (index < 0) return [...turns, next];
  const copy = [...turns];
  copy[index] = next;
  return copy;
}

function updateTurnItem(turns: Turn[] = [], turnId: string, item: ThreadItem) {
  return ensureTurn(turns, turnId).map((turn) => {
    if (turn.id !== turnId) return turn;
    const index = turn.items.findIndex((current) => current.id === item.id);
    const items = [...turn.items];
    if (index < 0) items.push(item);
    else items[index] = item;
    return { ...turn, items };
  });
}

function appendAgentDelta(turns: Turn[] = [], turnId: string, itemId: string, delta: string) {
  const normalized = ensureTurn(turns, turnId);
  return normalized.map((turn) => {
    if (turn.id !== turnId) return turn;
    const index = turn.items.findIndex((item) => item.id === itemId);
    const items = [...turn.items];
    if (index < 0) items.push({ type: "agentMessage", id: itemId, text: delta });
    else items[index] = { ...items[index], text: String(items[index].text || "") + delta };
    return { ...turn, items };
  });
}

function appendOutputDelta(turns: Turn[] = [], turnId: string, itemId: string, delta: string) {
  const normalized = ensureTurn(turns, turnId);
  return normalized.map((turn) => {
    if (turn.id !== turnId) return turn;
    const items = turn.items.map((item) => item.id === itemId ? { ...item, aggregatedOutput: String(item.aggregatedOutput || "") + delta } : item);
    return { ...turn, items };
  });
}

function ensureTurn(turns: Turn[] = [], turnId: string) {
  if (turns.some((turn) => turn.id === turnId)) return turns;
  return [...turns, { id: turnId, items: [], status: "inProgress", error: null, startedAt: null, completedAt: null, durationMs: null }];
}

function shortPath(path: string) {
  if (!path) return "フォルダーなし";
  const parts = path.replaceAll("\\", "/").split("/").filter(Boolean);
  return parts.at(-1) || path;
}

function groupThreads(threads: CodexThread[]) {
  const groups = new Map<string, CodexThread[]>();
  for (const thread of threads) {
    const label = shortPath(thread.cwd) || "作業フォルダーなし";
    const group = groups.get(label) || [];
    group.push(thread);
    groups.set(label, group);
  }
  return Array.from(groups, ([label, groupedThreads]) => ({ label, threads: groupedThreads }));
}

function endpointHost(value: string) {
  try { return new URL(value).host; } catch { return "App Server"; }
}

function defaultConnectionLabel(info: InitializeResponse, existing: ManagedConnection[]) {
  const platform = info.platformOs || info.platformFamily || "Remote";
  const base = `${platform} Codex`;
  const duplicates = existing.filter((connection) => connection.label === base || connection.label.startsWith(`${base} `)).length;
  return duplicates ? `${base} ${duplicates + 1}` : base;
}

function effortLabel(value: string) {
  const labels: Record<string, string> = {
    none: "なし",
    minimal: "最小",
    low: "軽",
    medium: "中程度",
    high: "高い",
    xhigh: "非常に高い",
    max: "最大",
    ultra: "Ultra",
  };
  return labels[value] || value;
}

function compactModelName(value: string) {
  return value.trim().replace(/^GPT-/iu, "");
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
