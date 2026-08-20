# App Serverプロトコル境界

- `.generated`はインストール済みCodex CLIから生成する参照専用成果物で、手編集しない。
- Host Agentだけが`initialize` / `initialized`を所有する。
- Browser APIはmethod allowlistを通す。任意JSON-RPC proxyとして公開しない。
- JSONLは32 MiBを上限とし、改行前に上限超過を検出して接続を終了する。
- Browser request IDはHost Agentで置換し、Browser sessionへ対応付ける。
- Server Request responseは、要求を受け取ったBrowser sessionだけが一度返せる。
- `account/chatgptAuthTokens/refresh`、`attestation/generate`など秘密情報を扱う要求はBrowserへ渡さない。

現在の`.generated`はCLI 0.147.0の再生成結果より古いことを確認している。差分が大きいため、この変更では上書きせず、別レビュー単位で再生成する。
