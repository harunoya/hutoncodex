# Design QA: Codex風Web UI

## Evidence

- Source visual truth: `C:\Users\hgzt23678\AppData\Local\Temp\codex-shot-2026-08-20_14-03-03.png`
- Source pixels: 1438 × 900
- Source state: Codex Desktopのダークテーマ、タスク選択済み、会話表示中
- Implementation URL: `http://127.0.0.1:8787/`
- Intended implementation viewport: 1280 × 820 CSS px、device scale 1
- Implementation screenshot: 取得できず
- Density normalization: 未実施。実装画像を取得できなかったため比較不能

## Full-view comparison

参照画像は開いて確認した。
Codex Desktopの左サイドバー、細いヘッダー、中央寄せの会話、下部コンポーザーを実装へ反映した。

実装ページはビルド済みのGatewayから配信したが、アプリ内ブラウザのURL安全ポリシーがローカルタブの自動再読み込みを拒否した。
同一状態・同一viewportの実装スクリーンショットがないため、視覚比較は完了していない。

## Focused region comparison

実装スクリーンショットを取得できていないため、次の領域は未比較。

- サイドバーの文字密度、行高、選択背景
- 会話本文の行長、Markdown、コードブロック
- コンポーザーの高さ、境界、モデル選択、送信ボタン
- 800px以下のドロワーと430px幅の操作領域

## Findings

- [P1] ブラウザ表示の目視確認が未完了
  - Location: Web UI全体
  - Evidence: 参照画像は存在するが、更新後実装のブラウザ画像がない
  - Impact: CSSの実表示、折返し、viewport内の収まりを完了判定できない
  - Fix: ユーザーがローカルタブを再読み込みした後、1280 × 820と393 × 852で画面を取得し、参照画像と比較する

## Implemented constraints

- App Serverで取得したHost、タスク、モデル、推論レベルだけを表示する
- 新規タスクは安全なWorkspace選択APIがないため追加しない
- レビュー、サブエージェント管理、設定画面は操作経路がないため追加しない
- コマンド実行とファイル変更だけ、定義済みJSON-RPC decisionで承認・拒否できる
- 未対応Server Requestは操作不能カードを出さず、JSON-RPCエラーで終了する

## Comparison history

### Iteration 1

- Earlier findings: 旧Web UIは白背景、未分類タスク一覧、JSONそのままの会話カード、固定フォームで、Codex Desktopの構造と大きく異なっていた
- Fixes made: ダークテーマ、Workspace別サイドバー、Markdown会話、活動行、モデル／推論選択付きコンポーザー、モバイルドロワーへ置換した
- Post-fix evidence: TypeScriptビルドとUI状態ヘルパーテストは成功。ブラウザ画像の取得はURL安全ポリシーでブロック

## Automated checks

- `npm test`: 41 tests passed
- `npm run build`: passed
- `cargo test --workspace`: 18 tests passed
- Browser console errors: 未確認
- Primary interactions: 自動操作未確認

## Implementation checklist

- [x] App Serverで利用できないナビゲーションを追加しない
- [x] タスクをWorkspace別に分類する
- [x] Markdown、コマンド、ファイル変更、ストリーミング文を別表示する
- [x] モデルと推論レベルをApp Server catalogに限定する
- [x] `turn/interrupt`へ取得済み`turnId`を渡す
- [x] 800px以下でサイドバーをドロワー化する
- [ ] 実装画面を1280 × 820で再撮影する
- [ ] 393 × 852でレスポンシブ表示を確認する
- [ ] 参照画像と実装画像を同時比較する

final result: blocked
