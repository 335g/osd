# osd

AI の長い回答を、付箋と図で噛み砕いて理解するための白板アプリ。
[understand-mode](https://github.com/fkubota/understand-mode) のコンセプトを Pi 向けに作り直したもの。

設計の経緯と決定事項は `CONTEXT.md`（ローカル管理・追跡外）。

## 構成

| パス | 役割 |
|---|---|
| `crates/osd-api` | ブリッジ API の型（サーバと UI で共有） |
| `crates/osd-server` | ブリッジ API サーバ。1 セッションに絞り、`pi` をフォークして質問に答える |
| `crates/osd-server/public` | Web 白板 UI（素の JS。understand-mode の `public/` を移植） |
| `extensions/pi-understand` | Pi 拡張（`/understand`）。ブリッジ起動→ブラウザ起動→サマリーを本体へ送る |

## 配布（他のマシンで使う）

```sh
cargo install --git https://github.com/335g/osd osd-server
```

白板 UI（`public/`）はバイナリに埋め込まれるので、`osd-server` 単体で動く。
Pi 拡張 `pi-understand` はこのリポジトリの `extensions/pi-understand` にある。
`settings.json` の packages に `"git:github.com/335g/osd"` を足して `/understand` を実行する。

## osd-server

```sh
cargo run -p osd-server -- --session <セッションIDかファイル> [--dir <作業フォルダ>] [--cwd <プロジェクト>] [--host 127.0.0.1] [--port 8765]
```

`--session` は ID（前方一致）でもファイルのパスでもよい。省略時は `PI_SESSION_FILE`。
1 つのセッションだけを開き、その中のアシスタント発言を扱う。プロジェクトやセッションの
一覧は持たない（headful で動いているセッションを 1 つ指定して開く使い方）。

起動すると 1 行目に URL を出す。白板 UI（または `pi-understand` 拡張）はこれを読む。

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

## Web UI（public/）

ブラウザで使う白板。`osd-server` が起動時に出す URL を開くと `public/` が表示される
（静的ファイルは合言葉なしで配り、API だけ認証する）。

- 本筋（最後のアシスタント発言）を全面に表示。文中を選ぶと付箋が生える
- 単語をダブルクリックすると、その場に定義が開く（「付箋にする」で深掘り）
- 付箋アクション: `question` / `detail` / `term` / `diagram` / `review`
- 「理解を終える」→ `return.md` に返す文を書き、サーバが終了する
- 素の JS + vendor（marked / DOMPurify / mermaid / vega）。ビルド工程なし

v1 は SQL 実行（`data`）と Vega-Lite を持たない（understand-mode 由来のコードは残るが未接続）。
