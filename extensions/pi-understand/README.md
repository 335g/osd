# pi-understand

直前のアシスタント回答を、付箋と図の白板（[osd](https://github.com/335g/osd)）で噛み砕いて理解する Pi 拡張。

## 使い方

```sh
export OSD_SERVER_BIN="$HOME/Dev/osd/target/debug/osd-server"
```

Pi を起動し直して `/understand` を実行すると：

1. osd-server（ブリッジ）が起動し、ブラウザに白板が開く
2. 本筋の文中を選んで付箋（質問・詳しく・用語・図・レビュー）を生やせる
3. 「理解を終える」→「全付箋をサマリーして次のプロンプトを作る」→「メインに送って閉じる」
4. サマリーが本体セッションへ次のプロンプトとして送られ、セッションが続く

本体セッションは汚れない。寄り道の質問は Pi のフォークで答える。

## 環境変数

- `OSD_SERVER_BIN` — osd-server のパス（既定 `osd-server`）

## 他のマシンへの導入

1. `cargo install --git https://github.com/335g/osd osd-server`（UI はバイナリに埋め込み済み）
2. Pi の `settings.json` の packages に `"git:github.com/335g/osd"` を追加
3. Pi を再起動して `/understand`

## 前提

osd-server バイナリが必要（`cargo install` か `cargo build -p osd-server` で作る）。白板の UI は osd-server が配る。
