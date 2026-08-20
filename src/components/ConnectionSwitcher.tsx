import {
  Check,
  ChevronRight,
  Code2,
  Keyboard,
  Plus,
  QrCode,
  Server,
  Unplug,
  X,
} from "lucide-react";
import { useRef } from "react";
import type { ManagedConnection } from "../types";
import { useModalFocus } from "../lib/mobileUi";

type ConnectionSwitcherProps = {
  open: boolean;
  connections: ManagedConnection[];
  activeId: string | null;
  onClose: () => void;
  onSelect: (id: string) => void;
  onAdd: () => void;
  onDisconnect: (id: string) => void;
};

export function ConnectionSwitcher({
  open,
  connections,
  activeId,
  onClose,
  onSelect,
  onAdd,
  onDisconnect,
}: ConnectionSwitcherProps) {
  const dialogRef = useRef<HTMLElement | null>(null);
  useModalFocus(dialogRef, open);

  if (!open) return null;
  const connectedCount = connections.filter((connection) => connection.state === "connected").length;

  return (
    <div className="connection-switcher-backdrop" onPointerDown={(event) => event.target === event.currentTarget && onClose()}>
      <section ref={dialogRef} tabIndex={-1} className="connection-switcher" role="dialog" aria-modal="true" aria-labelledby="connection-switcher-title">
        <header className="connection-switcher-heading">
          <div>
            <h2 id="connection-switcher-title">接続先</h2>
            <p>{connectedCount}台のCodexに接続中</p>
          </div>
          <button type="button" className="icon-button" onClick={onClose} aria-label="接続先を閉じる"><X size={18} /></button>
        </header>

        <div className="connection-switcher-list">
          {connections.map((connection) => {
            const selected = connection.id === activeId;
            return (
              <article className={`managed-connection ${selected ? "is-active" : ""}`} key={connection.id}>
                <button type="button" className="managed-connection-main" aria-current={selected ? "true" : undefined} aria-label={`${connection.label}、${connectionSubtitle(connection)}${selected ? "、使用中" : ""}`} onClick={() => onSelect(connection.id)}>
                  <span className={`managed-connection-icon state-${connection.state}`}>
                    {connection.mode === "manual" ? <Keyboard size={17} /> : connection.mode === "qr" ? <QrCode size={17} /> : <Code2 size={17} />}
                    <i aria-hidden="true" />
                  </span>
                  <span className="managed-connection-copy">
                    <strong>{connection.label}</strong>
                    <small>{connectionSubtitle(connection)}</small>
                  </span>
                  {selected ? <span className="managed-connection-current"><Check size={14} /> 使用中</span> : <ChevronRight size={15} />}
                </button>
                <button type="button" className="managed-connection-disconnect" onClick={() => onDisconnect(connection.id)} title={`${connection.label}を切断`} aria-label={`${connection.label}を切断`}><Unplug size={15} /></button>
              </article>
            );
          })}
          {!connections.length && (
            <div className="connection-switcher-empty">
              <Server size={21} />
              <strong>接続先はありません</strong>
              <span>PairコードまたはQR PairでCodexを追加できます。</span>
            </div>
          )}
        </div>

        <footer className="connection-switcher-footer">
          <button type="button" className="primary-button" onClick={onAdd}><Plus size={16} /> 接続を追加</button>
        </footer>
      </section>
    </div>
  );
}

function connectionSubtitle(connection: ManagedConnection) {
  const mode = connection.mode === "manual" ? "Pairコード" : connection.mode === "qr" ? "QR Pair" : "直接接続";
  const platform = connection.serverInfo?.platformOs || connection.endpoint || "Codex App Server";
  if (connection.state === "connecting") return `${mode} · 接続中`;
  if (connection.state === "error") return `${mode} · ${connection.detail || "接続エラー"}`;
  if (connection.state === "disconnected") return `${mode} · オフライン`;
  if (connection.detail?.includes("操作待ち")) return `${mode} · ${connection.detail}`;
  return `${mode} · ${platform}`;
}
