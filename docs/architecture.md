# Webアーキテクチャ

## コンポーネント

```text
Browser
  | HTTPS: login, host list, bounded commands
  | WSS: owned events, one-time ticket
  v
Rust Gateway
  | user/session/CSRF/origin/host/generation/request ownership
  | outbound Agent connection only
  v
Rust Host Agent
  | workspace allowlist, JSON-RPC ID rewriting, server-request ownership
  | stdio JSONL with line and queue limits
  v
codex app-server --stdio
```

GatewayはApp Serverの認証情報を保有しない。Host Agent用bootstrap tokenは現段階の開発用であり、本番化前に一回限り登録とローテーション可能なHost identityへ置換する。

## 初期化所有者

Host AgentがApp Server起動直後に`initialize`を要求し、成功後にparamsなしの`initialized`を一度だけ送る。Browserから同じRPCを送ることは禁止する。これにより、複数Browser sessionによる二重初期化を防ぐ。

## メッセージ所有権

Browser request IDはHost Agentで衝突しないIDへ置換し、応答時に元のIDとBrowser sessionへ戻す。thread IDを持つ通知とServer Requestは、そのthreadを操作したBrowser sessionへ関連付ける。秘密情報を扱う`account/chatgptAuthTokens/refresh`と`attestation/generate`はBrowserへ転送しない。

## Luna Max

開発作業用のLunaサブエージェントと製品機能のLuna Maxは別物である。製品機能は実行中App Serverの`model/list`を全ページ取得し、`model == "gpt-5.6-luna"`かつ`reasoningEffort == "max"`を確認した場合だけ有効にする。未確認、欠落、timeout時は利用不可として理由を表示し、別モデルへ切り替えない。

## 現在の未完了境界

- 永続DB、migration、複数ユーザー登録
- one-time Host enrollment、mTLSまたは非エクスポートHost鍵、token rotation/revocation
- Workspace IDからcanonical rootへの強制とsymlink/junction再検証
- App Server生成schemaによるRPC params/result検証
- Browser UI全体のGateway transport切替
- 再接続後のServer Request再同期と監査ログ
