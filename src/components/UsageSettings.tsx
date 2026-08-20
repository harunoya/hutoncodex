import {
  Bell,
  ChartNoAxesColumnIncreasing,
  ChevronRight,
  CircleGauge,
  GitBranch,
  Palette,
  Plug,
  RefreshCw,
  Settings2,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { useMemo, useRef, type ReactNode } from "react";
import { useModalFocus } from "../lib/mobileUi";
import type {
  AccountRateLimitsResponse,
  AccountTokenUsageResponse,
  RateLimitSnapshot,
  RateLimitWindow,
} from "../types";

type UsageSettingsProps = {
  open: boolean;
  connected: boolean;
  loading: boolean;
  error: string;
  rateLimits: AccountRateLimitsResponse | null;
  tokenUsage: AccountTokenUsageResponse | null;
  onClose: () => void;
  onRefresh: () => void;
};

type UsageRow = {
  key: string;
  label: string;
  compactLabel: string;
  description: string | null;
  remainingPercent: number;
};

const NAV_ITEMS = [
  { label: "一般", icon: Settings2 },
  { label: "外観", icon: Palette },
  { label: "通知", icon: Bell },
  { label: "パーソナライズ", icon: SlidersHorizontal },
  { label: "Git", icon: GitBranch },
  { label: "MCP とプラグイン", icon: Plug },
  { label: "使用状況", icon: ChartNoAxesColumnIncreasing, selected: true },
];

export function UsageSettings({
  open,
  connected,
  loading,
  error,
  rateLimits,
  tokenUsage,
  onClose,
  onRefresh,
}: UsageSettingsProps) {
  const rows = useMemo(() => buildUsageRows(rateLimits), [rateLimits]);
  const snapshot = selectPrimarySnapshot(rateLimits);
  const planLabel = formatPlan(snapshot?.planType ?? null);
  const resetCredits = toFiniteNumber(rateLimits?.rateLimitResetCredits?.availableCount);
  const dailyUsage = tokenUsage?.dailyUsageBuckets?.slice(-7).reverse() ?? [];
  const dialogRef = useRef<HTMLElement | null>(null);
  useModalFocus(dialogRef, open);

  if (!open) return null;

  return (
    <div
      className="usage-settings-backdrop"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section ref={dialogRef} tabIndex={-1} className="usage-settings-dialog" role="dialog" aria-modal="true" aria-labelledby="usage-settings-title">
        <header className="usage-settings-titlebar">
          <h2 id="usage-settings-title">設定</h2>
          <button className="usage-settings-close" onClick={onClose} title="設定を閉じる" aria-label="設定を閉じる">
            <X size={16} />
          </button>
        </header>

        <div className="usage-settings-layout">
          <aside className="usage-settings-nav" aria-label="設定カテゴリー">
            {NAV_ITEMS.map(({ label, icon: Icon, selected }) => (
              <button key={label} className={selected ? "is-selected" : ""} aria-current={selected ? "page" : undefined}>
                <Icon size={16} />
                <span>{label}</span>
                {selected && <ChevronRight size={14} />}
              </button>
            ))}
          </aside>

          <main className="usage-settings-content">
            <div className="usage-settings-heading">
              <div>
                <h1>使用状況</h1>
                <p>Codexの利用制限、リセット時刻、アカウントの使用量を確認できます。</p>
              </div>
              <button className="usage-refresh-button" onClick={onRefresh} disabled={!connected || loading}>
                <RefreshCw className={loading ? "spin" : ""} size={14} />
                更新
              </button>
            </div>

            {!connected && !rateLimits ? (
              <div className="usage-empty-state">
                <CircleGauge size={20} />
                <div>
                  <strong>App Serverへ接続してください</strong>
                  <span>Pair接続すると、Codexの現在の利用制限がここに表示されます。</span>
                </div>
              </div>
            ) : loading && !rateLimits ? (
              <div className="usage-empty-state">
                <RefreshCw className="spin" size={18} />
                <strong>使用状況を読み込んでいます…</strong>
              </div>
            ) : error && !rateLimits ? (
              <div className="usage-empty-state is-error">
                <CircleGauge size={20} />
                <div>
                  <strong>使用状況を読み込めませんでした</strong>
                  <span>{error}</span>
                </div>
              </div>
            ) : (
              <div className="usage-sections">
                {planLabel && (
                  <UsageSection title="ご利用中のプラン">
                    <UsageInfoRow label={planLabel} description={null}>
                      <button className="usage-outline-button" aria-disabled="true">プランを見る</button>
                    </UsageInfoRow>
                  </UsageSection>
                )}

                <UsageSection title="一般的な利用制限">
                  {rows.length ? (
                    rows.map((row) => <UsageLimitRow key={row.key} row={row} />)
                  ) : (
                    <UsageInfoRow label="利用制限の情報はありません" description={null} />
                  )}
                </UsageSection>

                {snapshot?.credits?.hasCredits && (
                  <UsageSection title="クレジット残高">
                    <UsageInfoRow label={snapshot.credits.unlimited ? "無制限" : formatCreditBalance(snapshot.credits.balance)} description="現在の残高" />
                  </UsageSection>
                )}

                {resetCredits > 0 && (
                  <UsageSection title="利用上限のリセット">
                    <UsageInfoRow label="利用可能なリセット" description="Codexの利用制限に使用できます">
                      <span className="usage-value">{resetCredits.toLocaleString("ja-JP")}回</span>
                    </UsageInfoRow>
                  </UsageSection>
                )}

                {dailyUsage.length > 0 && (
                  <UsageSection title="日別の使用状況" subtitle="使用量データは概算で、反映まで最大6時間かかる場合があります">
                    {dailyUsage.map((bucket) => (
                      <UsageInfoRow key={bucket.startDate} label={formatUsageDate(bucket.startDate)} description="トークン使用量">
                        <span className="usage-value">{formatNumber(bucket.tokens)}</span>
                      </UsageInfoRow>
                    ))}
                  </UsageSection>
                )}
              </div>
            )}
          </main>
        </div>
      </section>
    </div>
  );
}

export function getCompactUsage(rateLimits: AccountRateLimitsResponse | null) {
  return buildUsageRows(rateLimits).slice(0, 2);
}

function UsageSection({ title, subtitle, children }: { title: string; subtitle?: string; children: ReactNode }) {
  return (
    <section className="usage-section-card">
      <div className="usage-section-header">
        <h3>{title}</h3>
        {subtitle && <p>{subtitle}</p>}
      </div>
      <div className="usage-section-rows">{children}</div>
    </section>
  );
}

function UsageInfoRow({
  label,
  description,
  children,
}: {
  label: ReactNode;
  description: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="usage-info-row">
      <div className="usage-row-copy">
        <strong>{label}</strong>
        {description && <span>{description}</span>}
      </div>
      {children && <div className="usage-row-control">{children}</div>}
    </div>
  );
}

function UsageLimitRow({ row }: { row: UsageRow }) {
  return (
    <UsageInfoRow label={row.label} description={row.description}>
      <div className="usage-limit-control">
        <progress max={100} value={row.remainingPercent} aria-label={`${row.label}の残り利用可能量`} />
        <span>残り{Math.round(row.remainingPercent)}%</span>
      </div>
    </UsageInfoRow>
  );
}

function buildUsageRows(rateLimits: AccountRateLimitsResponse | null): UsageRow[] {
  const snapshot = selectPrimarySnapshot(rateLimits);
  if (!snapshot) return [];
  return [snapshot.primary, snapshot.secondary]
    .filter((window): window is RateLimitWindow => window != null)
    .map((window, index) => ({
      key: `${snapshot.limitId ?? "core"}-${index}`,
      label: formatWindowLabel(window.windowDurationMins),
      compactLabel: formatCompactWindowLabel(window.windowDurationMins),
      description: window.resetsAt ? `リセット：${formatResetTime(window.resetsAt)}` : null,
      remainingPercent: clampPercent(100 - window.usedPercent),
    }));
}

function selectPrimarySnapshot(rateLimits: AccountRateLimitsResponse | null): RateLimitSnapshot | null {
  if (!rateLimits) return null;
  const buckets = rateLimits.rateLimitsByLimitId;
  return buckets?.codex ?? (buckets ? Object.values(buckets)[0] : null) ?? rateLimits.rateLimits;
}

function formatWindowLabel(minutes: number | null) {
  if (minutes == null) return "利用制限";
  if (near(minutes, 300)) return "5時間の使用制限";
  if (near(minutes, 1440)) return "1日の利用制限";
  if (near(minutes, 10080)) return "週間利用制限";
  if (near(minutes, 43200)) return "月間利用制限";
  if (minutes < 1440) return `${Math.max(1, Math.round(minutes / 60))}時間の使用制限`;
  return `${Math.max(1, Math.round(minutes / 1440))}日の利用制限`;
}

function formatCompactWindowLabel(minutes: number | null) {
  if (minutes == null) return "利用制限";
  if (near(minutes, 300)) return "5時間";
  if (near(minutes, 1440)) return "1日";
  if (near(minutes, 10080)) return "週間";
  if (near(minutes, 43200)) return "月間";
  return minutes < 1440 ? `${Math.round(minutes / 60)}時間` : `${Math.round(minutes / 1440)}日`;
}

function formatResetTime(timestamp: number) {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const time = new Intl.DateTimeFormat("ja-JP", { hour: "2-digit", minute: "2-digit" }).format(date);
  if (date.toDateString() === now.toDateString()) return `今日 ${time}`;
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  if (date.toDateString() === tomorrow.toDateString()) return `明日 ${time}`;
  return new Intl.DateTimeFormat("ja-JP", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(date);
}

function formatPlan(plan: string | null) {
  if (!plan || plan === "unknown") return null;
  const labels: Record<string, string> = {
    free: "無料プラン",
    go: "Go プラン",
    plus: "Plus プラン",
    pro: "Pro プラン",
    prolite: "Pro プラン",
    team: "Team プラン",
    self_serve_business_usage_based: "Business プラン",
    business: "Business プラン",
    enterprise_cbp_usage_based: "Enterprise プラン",
    enterprise: "Enterprise プラン",
    edu: "Edu プラン",
  };
  return labels[plan] ?? plan;
}

function formatCreditBalance(balance: string | null) {
  if (!balance) return "0 クレジット";
  const value = Number(balance);
  return `${Number.isFinite(value) ? value.toLocaleString("ja-JP", { maximumFractionDigits: 2 }) : balance} クレジット`;
}

function formatUsageDate(date: string) {
  const value = new Date(`${date}T00:00:00`);
  return Number.isNaN(value.getTime()) ? date : new Intl.DateTimeFormat("ja-JP", { month: "short", day: "numeric", weekday: "short" }).format(value);
}

function formatNumber(value: number | string) {
  const numeric = Number(value);
  return Number.isFinite(numeric) ? numeric.toLocaleString("ja-JP") : String(value);
}

function toFiniteNumber(value: number | string | undefined) {
  const numeric = Number(value ?? 0);
  return Number.isFinite(numeric) ? numeric : 0;
}

function near(value: number, target: number) {
  return Math.abs(value - target) <= target * 0.05;
}

function clampPercent(value: number) {
  return Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : 0;
}
