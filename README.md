# osd

AI の長い回答を、付箋と図で噛み砕いて理解するためのネイティブアプリ。
[understand-mode](https://github.com/fkubota/understand-mode) のコンセプトを Pi 向けに作り直したもの。

設計の経緯と決定事項は `CONTEXT.md`（ローカル管理・追跡外）。

## 構成

| クレート | 役割 |
|---|---|
| `crates/osd-api` | ブリッジ API の型（サーバと GUI で共有） |
| `crates/osd-server` | ブリッジ API サーバ。1 セッションに絞り、`pi` をフォークして質問に答える |
| `crates/osd-app` | Rust 製 GUI（[rune](https://github.com/anomalyco/rune)）。発言の一覧と本文 |

## osd-server

```sh
cargo run -p osd-server -- --session <セッションIDかファイル> [--dir <作業フォルダ>] [--cwd <プロジェクト>] [--host 127.0.0.1] [--port 8765]
```

`--session` は ID（前方一致）でもファイルのパスでもよい。省略時は `PI_SESSION_FILE`。
1 つのセッションだけを開き、その中のアシスタント発言を扱う。プロジェクトやセッションの
一覧は持たない（headful で動いているセッションを 1 つ指定して開く使い方）。

起動すると 1 行目に URL を出す。GUI（または `pi-understand` 拡張）はこれを読む。

```
osd: http://127.0.0.1:8765/?t=<合言葉>
```

起動すると、そのセッションの最後のアシスタント発言が `main.md`（本筋）になる。
`/api/open` で別の発言に切り替え、`/api/return` で `return.md` に書いてフォークを消し、終了する。

### API

すべて `?t=<合言葉>` か `x-osd-token` ヘッダが必要。Host も検査する。

| エンドポイント | 入力 | 出力 |
|---|---|---|
| `GET /api/init` | — | `{markdown, title, hasSession, project}` |
| `GET /api/comments` | — | `{project, session, comments: [{index, preview, chars}], note}` |
| `POST /api/open` | `{index?}` | `{markdown, ...}`。選んだアシスタント発言を本筋にする（index 省略＝最後） |
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
- `OSD_SESSIONS_DIR` — セッション置き場（既定 `~/.pi/agent/sessions`）

## osd-app

```sh
cargo run -p osd-app -- --dir <作業フォルダ>        # ウィンドウ
cargo run -p osd-app -- --url 'http://127.0.0.1:8765/?t=...'
cargo run -p osd-app -- --scene note --snap /tmp/osd.png   # ヘッドレスで見た目確認
```

ブリッジが開いている 1 セッションの**最後のアシスタント発言**を全面に出す。
左の発言一覧は `⌘B`（macOS 以外は Ctrl+B）かヘッダの「≡ 一覧」で開け閉めする。
一覧は新しい順で、選んだ発言が本文になる。

ブリッジの URL は `--url`、`--dir`、`OSD_URL`、カレントの `url.txt` の順に探す。
`--scene sample` はブリッジ無しで見た目を確かめる見本。

rune は pre-release なので rev を固定する。`Cargo.toml` の `[patch.crates-io]` は rune の
README「Depending on Rune」の指示どおり、rune と同じ rev で揃えている（更新時は両方まとめて）。
