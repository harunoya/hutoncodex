# Host Agent

Host Agentは開発端末で動作し、Gatewayへ外向き接続してローカルApp Serverを管理する。

```powershell
cargo run -p hutoncodex-agent -- doctor --json
cargo run -p hutoncodex-agent -- app-server probe
cargo run -p hutoncodex-agent -- workspaces list --workspace C:\src\project
$env:HUTONCODEX_HOST_TOKEN="development-token-at-least-32-characters"
cargo run -p hutoncodex-agent -- connect `
  --gateway ws://127.0.0.1:8787 `
  --host-id 11111111-1111-4111-8111-111111111111 `
  --display-name workstation `
  --workspace C:\src\project
```

非loopbackの`ws://`は拒否する。本番は`wss://`を使用する。Host tokenは引数やURLへ入れず環境変数から読み取る。現在のtokenは開発bootstrap用であり、正式なHost登録機構ではない。

Workspaceは存在する絶対ディレクトリだけをcanonicalizeして重複除去する。現段階ではthread RPCの`cwd`をWorkspace IDへ強制変換していないため、公開運用へ進めてはならない。
