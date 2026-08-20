# TauriからWebへの移行

1. Rust共通crate、Gateway、Host Agentを追加する。旧Tauriは維持する。
2. Web transportを追加し、login、Host一覧、event接続を独立検証する。
3. ReactのApp Server操作をtransport interfaceへ移し、task/turn/catalog/usageをWebで同等化する。
4. 承認、複数Host、再接続、モバイル画面、E2Eを通す。
5. 公式Pair、QR、Relay、Discord、Androidに代わる運用を個別判定する。
6. parity表が全項目合格してからTauri配布の扱いを決める。

削除禁止対象は`src-tauri`、Pair/enrollment/Relay、Android KeyStore、Discord Presence、現行React UI、既存テストである。
