# ローカル導入

## 開発用Gateway

```powershell
$env:CODEX_REMOTE_ADMIN_PASSWORD="change-this-development-password"
$env:CODEX_REMOTE_HOST_TOKEN="change-this-development-host-token-32chars"
cargo run -p codex-remote-gateway
```

別terminalでHost Agentを起動する。

```powershell
$env:CODEX_REMOTE_HOST_TOKEN="change-this-development-host-token-32chars"
cargo run -p codex-remote-agent -- connect `
  --gateway ws://127.0.0.1:8787 `
  --host-id 11111111-1111-4111-8111-111111111111 `
  --workspace C:\absolute\workspace
```

## 公開前の必須条件

- HTTPS reverse proxyと`--secure-cookies`
- `CODEX_REMOTE_PUBLIC_ORIGIN=https://exact-origin.example`
- one-time Host enrollmentとtoken rotation/revocation
- 永続User/Host/Workspace DBとmigration
- request/turn/user単位rate limit、監査、secret redaction
- canonical Workspace confinementとApp Server schema validation
- バックアップ、復旧、アップグレード、rollback手順

これらが未完了のため、現在のGatewayをインターネットへ公開してはならない。Tailscale内で試す場合もHTTPS、端末ACL、Gateway認証を省略しない。
