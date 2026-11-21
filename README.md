# osd

AI の長い回答を、付箋と図で噛み砕いて理解するためのネイティブアプリ。
[understand-mode](https://github.com/fkubota/understand-mode) のコンセプトを Pi 向けに作り直したもの。

設計の経緯と決定事項は `CONTEXT.md`（ローカル管理・追跡外）。

## 構成

| クレート | 役割 |
|---|---|
| `crates/osd-api` | ブリッジ API の型（サーバと GUI で共有） |
| `crates/osd-server` | ブリッジ API サーバ。`pi` をフォークして付箋の質問に答える |
| `crates/osd-app` | Rust 製 GUI（[rune](https://github.com/anomalyco/rune)）。本筋・付箋・返す文まで動く |

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
| `GET /api/sessions` | — | `{sessions: [{id, file, cwd, project, updated, preview}]}`（新しい順） |
| `POST /api/comments` | `{file}` | `{project, comments: [{index, preview, chars}]}`（そのセッションのアシスタント発言） |
| `POST /api/open` | `{file, index?}` | `{markdown, ...}`。選んだアシスタント発言を本筋にする（index 省略＝最後） |
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

## osd-app

```sh
cargo run -p osd-app -- --dir <作業フォルダ>        # ウィンドウ
cargo run -p osd-app -- --url 'http://127.0.0.1:8765/?t=...'
cargo run -p osd-app -- --scene note --snap /tmp/osd.png   # ヘッドレスで見た目確認
```

左に全セッションの一覧（プロジェクト名・更新時刻・最初の頼みごと）。セッションを選ぶと、
右上にそのセッションのアシスタント発言の一覧、右下に選んだ発言の本文が出る。
一覧は全プロジェクト横断（プロジェクトで絞る仕組みはまだ無い）。

ブリッジの URL は `--url`、`--dir`、`OSD_URL`、カレントの `url.txt` の順に探す。
`--scene sample` はブリッジ無しで見た目を確かめる見本。

rune は pre-release なので rev を固定する。`Cargo.toml` の `[patch.crates-io]` は rune の
README「Depending on Rune」の指示どおり、rune と同じ rev で揃えている（更新時は両方まとめて）。
