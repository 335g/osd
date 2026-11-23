/**
 * pi-understand: 直前のアシスタント回答を付箋の白板（osd）で噛み砕いて理解する。
 *
 * /understand で osd-server（ブリッジ）を起動してブラウザを開き、
 * 白板の「理解を終える」で書かれた return.md を本体セッションへ送り返す。
 *
 * 必要なもの: osd-server バイナリ。既定は PATH 上の `osd-server`。
 * `OSD_SERVER_BIN` 環境変数で差し替えられる（例: ~/Dev/osd/target/debug/osd-server）。
 */

import { spawn } from "node:child_process";
import { readFileSync, unwatchFile, watchFile } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const SERVER_BIN = process.env.OSD_SERVER_BIN ?? "osd-server";

function openBrowser(url: string): void {
  const [cmd, ...args] =
    process.platform === "darwin"
      ? ["open", url]
      : process.platform === "win32"
        ? ["cmd", "/c", "start", "", url]
        : ["xdg-open", url];
  spawn(cmd, args, { stdio: "ignore", detached: true }).unref();
}

export default function (pi: ExtensionAPI) {
  pi.registerCommand("understand", {
    description: "直前の回答を付箋の白板で理解する（終えたらサマリーを本体に返す）",
    handler: async (_args, ctx) => {
      const sessionFile = ctx.sessionManager.getSessionFile();
      if (!sessionFile) {
        ctx.ui.notify("セッションファイルが見つかりません。", "error");
        return;
      }

      // 起動ごとに固有の作業フォルダ。return.md の食い残りを避ける。
      const workDir = join(tmpdir(), `osd-${Date.now()}`);
      const returnFile = join(workDir, "return.md");

      const child = spawn(
        SERVER_BIN,
        ["--session", sessionFile, "--cwd", ctx.cwd, "--dir", workDir, "--port", "0"],
        { stdio: ["ignore", "pipe", "inherit"] },
      );

      let opened = false;
      let buf = "";
      child.stdout?.on("data", (chunk) => {
        if (opened) return;
        buf += chunk.toString();
        const m = buf.match(/osd: (http:\/\/[^\s]+)/);
        if (m) {
          opened = true;
          openBrowser(m[1]);
          ctx.ui.notify("白板を開きました。終えたら「理解を終える」でサマリーが本体に返ります。", "info");
        }
      });

      child.on("error", () => {
        ctx.ui.notify(
          `osd-server（${SERVER_BIN}）を起動できません。OSD_SERVER_BIN でパスを指定してください。`,
          "error",
        );
      });

      // return.md が書かれたら、その内容を本体セッションへ次のプロンプトとして送る。
      watchFile(returnFile, { interval: 500 }, () => {
        try {
          const text = readFileSync(returnFile, "utf-8").trim();
          if (!text) return;
          unwatchFile(returnFile);
          if (text === "(本筋に返すことはありません)") {
            ctx.ui.notify("白板を閉じました（本体へは何も送りません）。", "info");
            return;
          }
          pi.sendUserMessage(text);
          ctx.ui.notify("白板のサマリーを本体セッションへ送りました。", "info");
        } catch {
          // return.md がまだ書かれていない
        }
      });
    },
  });
}
