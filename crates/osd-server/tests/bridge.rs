//! ブリッジの自己点検。偽の pi（JSON イベントを吐くだけのシェル）で API とフォーク処理を検証する。
//! 実行: cargo test -p osd-server

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SESSION_ID: &str = "01a1fd25-d307-71ed-9276-f15514c05806";

struct Server {
    child: Child,
    origin: String,
    token: String,
    work: PathBuf,
}

impl Server {
    fn start() -> Server {
        let work = std::env::temp_dir().join(format!(
            "osd-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("main.md"), "# 本筋\n\n本文です。\n").unwrap();
        std::fs::write(work.join("title.txt"), "テスト\n").unwrap();

        // 偽の pi: 引数を記録し、--session-dir にフォークのファイルを作り、pi --mode json と同じ形のイベントを吐く
        let script = work.join("fake-pi.sh");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
set -e
ARGV="{work}/argv.txt"
: > "$ARGV"
DIR=""
prev=""
for a in "$@"; do
  printf '%s\n' "$a" >> "$ARGV"
  if [ "$prev" = "--session-dir" ]; then DIR="$a"; fi
  prev="$a"
done
mkdir -p "$DIR"
: > "$DIR/2026-01-01T00-00-00-000Z_{SESSION_ID}.jsonl"
echo '{{"type":"session","id":"{SESSION_ID}","parentSession":"parent.jsonl"}}'
echo '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"偽の答え"}}],"usage":{{"cost":{{"total":0.5}}}}}}}}'
"#,
                work = work.display()
            ),
        )
        .unwrap();
        make_executable(&script);

        let mut child = Command::new(env!("CARGO_BIN_EXE_osd-server"))
            .arg("--dir")
            .arg(&work)
            .arg("--session")
            .arg("parent.jsonl")
            .arg("--cwd")
            .arg(&work)
            .arg("--port")
            .arg("0")
            .env("OSD_PI_BIN", &script)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let url = line
            .trim()
            .strip_prefix("osd: ")
            .unwrap_or_else(|| panic!("起動の行が読めません: {line:?}"))
            .to_owned();
        let (origin, token) = url.split_once("/?t=").unwrap();
        Server {
            child,
            origin: origin.to_owned(),
            token: token.to_owned(),
            work,
        }
    }

    fn call(&self, method: &str, path: &str, token: &str, body: &str) -> (u16, String) {
        let addr = self.origin.trim_start_matches("http://");
        let mut stream = TcpStream::connect(addr).unwrap();
        let q = if token.is_empty() { "" } else { "?t=" };
        let sep = if path.contains('?') { "&" } else { "?" };
        let target = if token.is_empty() {
            path.to_owned()
        } else {
            format!("{path}{sep}{q}{token}")
        };
        let req = format!(
            "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nX-Osd-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, body.to_owned())
    }

    fn close(self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut child = self.child;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                std::fs::remove_dir_all(&self.work).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        child.kill().ok();
        panic!("API を閉じたのにサーバが終わりません");
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn init_ask_return() {
    let s = Server::start();

    let (code, body) = s.call("GET", "/api/init", &s.token, "");
    assert_eq!(code, 200);
    assert!(body.contains("本文です"), "本筋を返す: {body}");
    assert!(body.contains("\"hasSession\":true"), "セッションあり: {body}");

    let (code, _) = s.call("GET", "/api/init", "wrong", "");
    assert_eq!(code, 403, "合言葉が違えば断る");

    let (code, body) = s.call(
        "POST",
        "/api/ask",
        &s.token,
        r#"{"action":"detail","anchor":"対象の文"}"#,
    );
    assert_eq!(code, 200);
    assert!(body.contains("\"ok\":true"), "分身が答える: {body}");
    assert!(body.contains("偽の答え"), "回答本文: {body}");
    assert!(body.contains(SESSION_ID), "セッションID: {body}");
    assert!(body.contains("forkFile"), "続きの質問用のファイル: {body}");

    let argv = std::fs::read_to_string(s.work.join("argv.txt")).unwrap();
    assert!(argv.contains("--fork\nparent.jsonl"), "本体から分岐する: {argv}");
    assert!(argv.contains("--session-dir"), "フォークは作業フォルダに置く");
    assert!(argv.contains("bash,edit,write"), "編集系の道具を外す");
    assert!(argv.contains("対象の文"), "選んだ文がプロンプトに入る");

    let (code, body) = s.call("POST", "/api/return", &s.token, r#"{"text":"本筋に返す文"}"#);
    assert_eq!(code, 200);
    assert!(body.contains("\"ok\":true"), "{body}");
    assert_eq!(
        std::fs::read_to_string(s.work.join("return.md")).unwrap().trim(),
        "本筋に返す文"
    );
    assert!(
        !s.work.join("sessions").exists(),
        "フォークを消してから閉じる"
    );
    s.close();
}
