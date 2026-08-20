# Codex Remote 実装方針

更新日: 2026-08-20

## 2026-08-20の上位決定

Codex Remoteの主製品を、**Browser → Rust Gateway → outbound-only Rust Host Agent → loopback stdio Codex App Server**へ段階移行する。

- BrowserはGatewayのHttpOnlyセッションだけを使用し、Codex token、Host token、端末鍵、enrollmentを受け取らない。
- Host Agentは利用者が登録した絶対workspaceだけを公開し、GatewayからHostへの着信ポートを要求しない。
- App Serverの`initialize` / `initialized`はHost Agentが一度だけ実行する。Browserへ初期化RPCを開放しない。
- Browser操作はGatewayのメソッドallowlist、ユーザー・Host・接続世代・Browser session所有権を通過したものだけをHost Agentへ送る。
- 製品機能のLuna Maxは、実行中App Serverの`model/list`に`gpt-5.6-luna`と`max`の組がある場合だけ有効にする。別モデルへ暗黙フォールバックしない。
- 既存Tauri、公式Pair、QR Pair、Relay、Android資産は、Web版が同等の主要フローを満たすまで削除しない。

詳細は[Webアーキテクチャ](docs/architecture.md)と[ADR-0001](docs/adr/0001-web-gateway-host-agent.md)を正とする。

以下の2026-08-01方針は、移行中のTauri版を保守するための互換方針として残す。新規Web製品の外部接続方式を定義するものではない。

## 旧Tauri版の決定

Codex Remote は、**公式 Remote Control Pair を第一の接続方式とする Tauri クライアント**として開発する。Pair コードまたは QR Pair により、OpenAI が提供する enrollment と Relay を経由して接続する。

上級者向けの直接 WebSocket 接続は、ローカル開発、SSH ポートフォワード、または利用者が管理する TLS 終端済み App Server のためだけに維持する。通常導線、Android の代替導線、または Pair の失敗時フォールバックには使用しない。

外部プロジェクトは実装をコピーせず、次の範囲でのみ参照する。

| 参照先 | 採用する知見 | 採用しないもの |
| --- | --- | --- |
| [OpenAI Codex](https://github.com/openai/codex) | App Server 型、Remote Control の状態・Pair・Relayの意味、CLI認証の正式な入口 | 非公開のモバイルクライアント動作の推測 |
| [Remodex Android fork](https://github.com/Demogorgon314/remodex-android) | モバイルの接続管理、再接続、会話・承認UX | 独自Bridge、独自Relay、独自暗号プロトコル |
| [Pocodex](https://github.com/davej/pocodex) / [codex-web](https://github.com/0xcaff/codex-web) | ブラウザ対応の画面・再接続のUX | Desktop bundleの配信・Electron shim・LAN公開 |
| [Codex Remote Control Lab](https://github.com/Sunwood-ai-labs/codex-remote-control-lab) / [chenhaoc](https://github.com/chenhaoc/codex-remote-control) | Androidの画面遷移、テスト用mock | Token付きWebSocket Bridgeを通常接続にすること |

## 接続アーキテクチャ

```text
通常:     Tauri UI -> Rust Pair client -> 公式 enrollment / 公式 Relay -> Remote App Server
上級者:   Tauri UI -> Rust WebSocket client -> 利用者管理の App Server
禁止:     Tauri UI -> 自前Relay / LAN Bridge -> App Server  （通常接続の代替）
```

### 1. Rustを信頼境界にする

- Pair、端末鍵、enrollment、OAuthの追加認証、Relay、Relay envelope 検証は Rust のみで扱う。
- React は Pair コード、QRの値、進行状況、接続結果だけを型付き Tauri command/event で扱う。アクセストークン、Remote Control token、端末鍵、enrollment の中身を受け取らない。
- `.generated/protocol-ts` は App Server の要求・応答型の唯一の参照元とし、直接編集しない。
- HTTP Client は共有し、接続キャンセル、タイムアウト、接続世代、Relayサイズ上限・宛先検証を省略しない。

### 2. 認証をCodex CLIの正式インターフェースへ寄せる

- `auth.json` の直接読取と、人間向け `codex login --device-auth` 出力の解析を恒久的な認証APIにしない。
- Rust に `CodexAuthBroker` を置き、ローカル `codex app-server` の `account/read`、`getAuthStatus`、`account/login/start`、`account/login/cancel`、`account/login/completed` を使ってログイン状態とデバイスコードフローを扱う。
- 資格情報ストアは Codex CLI の設定を尊重する。file保存を強制せず、Token・refresh token・Pairコードをログ、Tauri event、Discord Presence、永続UI状態へ出さない。
- Remote Control用の追加認証は通常のChatGPTログインとは別段階として扱い、公式フローの実機確認が取れるまで既存の検証を緩めない。

### 3. 接続方式を混在させない

- `Pair`、`QR Pair`、`Advanced WebSocket` は異なる `ConnectionTransport` として保存する。
- Pairが失敗したときに直接WebSocket、Tailscale URL、独自Bridgeへ黙って切り替えない。利用者が上級者向けを明示選択した場合だけ直接接続する。
- 接続ごとにID、表示名、transport、接続世代、phase、active thread、draft、busy、models、usage、pending server requests を保持する。
- すべての非同期応答・通知は接続ID、thread ID、世代を照合してから状態へ反映する。

## プラットフォーム方針

### Windows desktop: 正式サポート

- Windows CNG の非エクスポート P-256 鍵を公式Pairの端末鍵として使用する。
- Pair、QR、Relay、直接WebSocket、Discord Presence をサポートする。
- Windows鍵の消失または署名不能は stale enrollment として安全に破棄し、再登録へ進める。

### Android: 段階的サポート

- Android版はAndroid KeyStoreによるP-256生成、非エクスポート性、SPKI公開鍵取得、SHA-256 ECDSA DER署名を実装する。起動時セルフテストに失敗した端末ではPairボタンを無効化する。
- Codex認証は公開デバイスコードフローをRustで処理し、TokenはKeyStore内のAES-GCM鍵で暗号化する。React、ログ、Preferencesの平文へ渡さない。
- Android Emulatorで端末鍵、暗号化保存、デバイスコード発行、ブラウザ起動、待機、キャンセルを検証する。実アカウントでenrollment、Pair claim、Relayまで完了するまではAndroid Pairを正式サポートと主張しない。
- 平文秘密鍵、独自鍵形式、Remodex互換鍵、直接WebSocketへの自動フォールバックを追加しない。
- AndroidのDiscord Presenceはno-opを維持する。

## UIと運用の方針

- 通常の接続画面は Pairコード、QR Pair の順に見せる。直接 WebSocket は「上級者向け」に隔離する。
- 接続途中は最前面のモーダルだけを閉じられる。カメラ許可待ち・Pair進行中にキャンセル不能なら、閉じる、Escape、タブ切替、背景クリックを一貫して無効にする。
- QR scanner、MediaStreamTrack、接続試行、Tauri listener、Relay task は画面終了・切断・世代失効時に確実に停止する。
- 多数接続、同名接続、接続切替、バックグラウンドのturn/approval/usage通知を前提にし、状態をグローバルに共有しない。
- APKは正規keystoreで署名して配布する。Tailscale配布は信頼されたtailnet内の配布経路であり、認証方式の代替にはしない。

## 実装順序

### Phase 1 — 接続基盤を正す（最優先）

1. `CodexAuthBroker` を導入し、直接auth.json読取・CLI標準出力解析を削除する。
2. Pairのキャンセル、操作世代、Relay envelope検証、分割メッセージ上限、stale enrollment復旧をRustテストで固定する。
3. Pair、QR、直接接続の状態機械を明確にし、失敗時のtransport混在を禁止する。

### Phase 2 — 状態整合性

1. 接続単位のdraft、busy、thread、model、usage、pending request を単一の接続状態モデルへ集約する。
2. `turn/completed` の不完全通知は既存itemsを安全にマージし、必要時だけ世代付き `thread/resume` を実行する。
3. Server Request は所有接続へ応答し、別クライアントで解決された通知で削除する。

### Phase 3 — Android Pairの可否判定

1. `DeviceIdentity` を Rust trait として抽象化し、Windows CNGとAndroid KeyStoreの実装を分離する。
2. Android KeyStoreのP-256、非エクスポート性、SPKI、DER署名、暗号化保存を計装テストと起動時セルフテストで固定する。
3. セルフテスト成功時だけPair機能フラグを有効化する。実アカウントでPair、QR、再接続、鍵消失復旧、バックグラウンド復帰を確認するまでは正式サポートとしない。

### Phase 4 — 体験と配布

1. Androidで360px相当の接続、タスク選択、承認、送信、usageを実画面確認する。
2. Discord Presenceはデスクトップのみで維持し、接続処理から独立させる。
3. release APKを縮小・署名し、Tailscale配布時はハッシュと更新手順を併記する。

## 受入基準

- `npm test`、`npm run build`、`cargo fmt -- --check`、`cargo clippy -- -D warnings`、`cargo test` が成功する。
- Windowsで公式PairコードとQR Pairを実機確認する。認証、Pair、Relay、再接続、複数接続、切断を含める。
- Android Pairはセルフテスト成功端末で試験利用できるが、実機の公式環境でenrollment、Pair claim、Relayまで成功するまでは「正式対応済み」と主張しない。previewやmockだけを実機確認として扱わない。
- すべての接続切替・通知・Server Request・送信失敗で、別接続の表示または入力が混入しない。
- 公式プロトコルの変更時は、生成型の更新、Pair実機確認、Relayの不正envelope／過大segmentテストを必須にする。

## 明確に行わないこと

- Remodex、Pocodex、codex-webなどの独自BridgeまたはRelayを公式Pairの代替として組み込まない。
- Pairの失敗を隠す自動フォールバックを追加しない。
- Android対応のために秘密鍵をファイル、localStorage、Preferencesへ保存しない。
- private APIの推測だけでプロトコル検証を外さない。
