export type JsonId = string | number;

export type JsonRpcError = {
  code: number;
  message: string;
  data?: unknown;
};

export type WireMessage = {
  id?: JsonId;
  method?: string;
  params?: Record<string, unknown>;
  result?: unknown;
  error?: JsonRpcError;
};

export type ConnectionStatus = {
  connectionId: number;
  state: "connected" | "disconnected" | "error";
  detail?: string;
};

export type IncomingEnvelope = {
  connectionId: number;
  message: WireMessage;
};

export type PairingProgress = {
  attemptId: string;
  stage: "auth" | "device" | "authorize" | "pair" | "environment" | "relay";
  detail: string;
  verificationUrl?: string;
  userCode?: string;
};

export type ConnectionTiming = {
  attemptId: string;
  phase: string;
  elapsedMs: number;
  buildProfile: "debug" | "release";
};

export type ThreadStatus =
  | { type: "notLoaded" | "idle" | "systemError" }
  | { type: "active"; activeFlags: string[] };

export type UserInput =
  | { type: "text"; text: string; text_elements: unknown[] }
  | { type: "image"; url: string; detail?: string }
  | { type: "localImage"; path: string; detail?: string }
  | { type: "skill" | "mention"; name: string; path: string };

export type ThreadItem = {
  type: string;
  id?: string;
  text?: string;
  content?: UserInput[] | string[];
  summary?: string[];
  command?: string;
  cwd?: string;
  status?: string;
  aggregatedOutput?: string | null;
  exitCode?: number | null;
  changes?: Array<Record<string, unknown>>;
  server?: string;
  tool?: string;
  arguments?: unknown;
  result?: unknown;
  error?: unknown;
  [key: string]: unknown;
};

export type Turn = {
  id: string;
  items: ThreadItem[];
  status: "inProgress" | "completed" | "failed" | "interrupted" | string;
  error: { message?: string } | null;
  startedAt: number | null;
  completedAt: number | null;
  durationMs: number | null;
};

export type CodexThread = {
  id: string;
  sessionId: string;
  preview: string;
  name: string | null;
  cwd: string;
  modelProvider: string;
  createdAt: number;
  updatedAt: number;
  recencyAt: number | null;
  status: ThreadStatus;
  turns: Turn[];
  gitInfo?: { branch?: string; repositoryUrl?: string } | null;
  [key: string]: unknown;
};

export type CodexModel = {
  id: string;
  model: string;
  displayName: string;
  description: string;
  hidden: boolean;
  isDefault: boolean;
  defaultReasoningEffort: string;
  supportedReasoningEfforts: Array<{
    reasoningEffort: string;
    description?: string;
  }>;
};

export type InitializeResponse = {
  userAgent: string;
  codexHome: string;
  platformFamily: string;
  platformOs: string;
};

export type ManagedConnectionMode = "manual" | "qr" | "advanced";

export type ManagedConnection = {
  id: string;
  connectionId: number;
  label: string;
  mode: ManagedConnectionMode;
  state: "connected" | "connecting" | "disconnected" | "error";
  detail?: string;
  endpoint?: string;
  serverInfo: InitializeResponse | null;
  createdAt: number;
};

export type ServerRequest = {
  id: JsonId;
  method: string;
  params: Record<string, unknown>;
};

export type RateLimitWindow = {
  usedPercent: number;
  windowDurationMins: number | null;
  resetsAt: number | null;
};

export type RateLimitSnapshot = {
  limitId: string | null;
  limitName: string | null;
  primary: RateLimitWindow | null;
  secondary: RateLimitWindow | null;
  credits: {
    hasCredits: boolean;
    unlimited: boolean;
    balance: string | null;
  } | null;
  planType: string | null;
  rateLimitReachedType: string | null;
};

export type AccountRateLimitsResponse = {
  rateLimits: RateLimitSnapshot;
  rateLimitsByLimitId: Record<string, RateLimitSnapshot> | null;
  rateLimitResetCredits: {
    availableCount: number | string;
    credits: Array<{
      id: string;
      status: string;
      grantedAt: number;
      expiresAt: number | null;
      title: string | null;
      description: string | null;
    }> | null;
  } | null;
};

export type AccountTokenUsageResponse = {
  summary: {
    lifetimeTokens: number | string | null;
    peakDailyTokens: number | string | null;
    longestRunningTurnSec: number | string | null;
    currentStreakDays: number | string | null;
    longestStreakDays: number | string | null;
  };
  dailyUsageBuckets: Array<{
    startDate: string;
    tokens: number | string;
  }> | null;
};
