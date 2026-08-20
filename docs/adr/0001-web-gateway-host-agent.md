# ADR-0001: Web Gatewayとoutbound Host Agentへ移行する

状態: Accepted（段階移行中）  
日付: 2026-08-20

## 文脈

現行Tauri版は端末内のRustを信頼境界にできる一方、別端末のブラウザから同じApp Serverを安全に操作する経路を持たない。App Serverのtokenや`auth.json`をBrowserへ渡したり、App ServerのWebSocketを公開したりすると、XSS、履歴、ログ、拡張機能、Origin処理が新しい秘密情報流出面になる。

## 決定

Browserは認証済みRust Gatewayだけへ接続する。Rust Host AgentはGatewayへ外向きWSS接続し、ローカルのApp Serverをstdioで一度だけ初期化する。GatewayはUser、Browser session、Host、接続世代を照合し、許可した操作だけを中継する。

生の資格情報、任意endpoint、任意filesystem pathはBrowser APIに含めない。WorkspaceはHost Agent起動時の絶対パスallowlistから識別子へ解決する。

既存Tauri版はWeb版が接続、task、turn、承認、usage、複数接続の同等性を満たすまで残す。

## 結果

- App Serverを公開ネットワークへbindする必要がない。
- Browserを秘密情報の信頼境界から外せる。
- GatewayとHost Agentの運用、TLS、登録、失効、監査が新たに必要になる。
- raw JSON-RPCの完全透過性より、method schemaとrequest ownershipの安全性を優先する。

## 却下した案

- BrowserからApp Serverへ直接WebSocket接続する案。
- Codex tokenまたはHost tokenをlocalStorageへ保存する案。
- Pair失敗時に独自Bridgeへ自動フォールバックする案。
- 旧Tauri資産を先に削除する一括移行案。
