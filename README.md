# osd

AI の長い回答を、付箋と図で噛み砕いて理解するためのネイティブアプリ。
[understand-mode](https://github.com/fkubota/understand-mode) のコンセプトを Pi 向けに作り直したもの。

設計の経緯と決定事項は `CONTEXT.md`（ローカル管理・追跡外）。

## 構成

| クレート | 役割 |
|---|---|
| `crates/osd-api` | ブリッジ API の型（サーバと GUI で共有） |
| `crates/osd-server` | ブリッジ API サーバ。`pi` をフォークして付箋の質問に答える |
| `crates/osd-app`（未着手） | Rust 製 GUI（[rune](https://github.com/anomalyco/rune)） |

## osd-server

```sh
cargo run -p osd-server -- --dir <作業フォルダ> --session <本体セッション> [--cwd <プロジェクト>] [--host 127.0.0.1] [--port 8765]
```

起動すると 1 行目に URL を出す。GUI（または `pi-understand` 拡張）はこれを読む。

```
osd: http://127.0.0.1:8765/?t=<合言葉>
```

`--dir` の中に `main.md`（本筋）と `title.txt` を置くと `/api/init` が返す。
`/api/return` で `return.md` に書き、フォークを消して終了する。

### API

すべて `?t=<合言葉>` か `x-osd-token` ヘッダが必要。Host も検査する。

| エンドポイント | 入力 | 出力 |
|---|---|---|
| `GET /api/init` | — | `{markdown, title, hasSession, project}` |
| `POST /api/ask` | `{action, anchor, question}` / `{continueSession, forkFile, followUp}` | `{ok, text, sessionId, forkFile, costUsd}` |
| `POST /api/draft` | `{notes}` | 同上（本筋に返す文の下書き） |
| `POST /api/return` | `{text}` | `{ok}` |

`action` は `question` / `detail` / `term` / `diagram` / `review`。
`data`（SQL 実行）と Vega-Lite は v1 では持たない。

## 検証

```sh
cargo test -p osd-server
```

偽の `pi`（JSON イベントを吐くだけのシェル）で、認証・分岐・後片付けを確認する。

## 環境変数

- `OSD_PI_BIN` — `pi` のパス（既定 `pi`）
- `OSD_TIMEOUT_SEC` — 1 問の待ち時間（既定 300）
