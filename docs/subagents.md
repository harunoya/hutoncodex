# サブエージェント設計

製品のLuna Max jobは、親thread、子thread、User、Host、generationへ結び付ける。実行前にApp Serverの全`model/list`ページから`gpt-5.6-luna`と`max`を検証する。

```text
queued -> running -> completed
                  -> failed
                  -> cancelled
                  -> unknownAfterDisconnect
```

親threadのモデルやeffortを暗黙に継承しない。Luna Maxが利用できない場合はjobを作らず、理由を返す。Gateway再起動やHost切断後に状態を推測でcompletedへ進めない。

現段階では型とcapability判定だけを実装しており、scheduler、永続化、cancel、再同期は未実装である。
