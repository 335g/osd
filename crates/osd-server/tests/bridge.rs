//! ブリッジの自己点検。偽の pi（JSON イベントを吐くだけのシェル）で API とフォーク処理を検証する。
//! 実行: cargo test -p osd-server

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SESSION_ID: &str = "01a1fd25-d307-71ed-9276-f15514c05806";
const OTHER_ID: &str = "01a1fd25-d307-71ed-9276-f15514c05807";

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

        // 見本のセッション置き場（本物の ~/.pi を触らずに一覧と open を試す）
        let sessions = work.join("pi-sessions");
        let slug = sessions.join("--tmp-project--");
        std::fs::create_dir_all(&slug).unwrap();
        let big = "x".repeat(40_000);
        std::fs::write(
            slug.join(format!("2026-01-01T00-00-00-000Z_{SESSION_ID}.jsonl")),
            format!(
                r#"{{"type":"session","id":"{SESSION_ID}","cwd":"/tmp/project"}}
{{"type":"message","message":{{"role":"system","content":"{big}"}}}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"最初の頼みごと"}}]}}}}
{{"type":"message","message":{{"role":"assistant","content":[{{"type":"text","text":"古い答え"}}]}}}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"次の頼みごと"}}]}}}}
{{"type":"message","message":{{"role":"assistant","content":[{{"type":"text","text":"最後の答え"}}]}}}}
"#
            ),
        )
        .unwrap();

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

        // 偽の hc: /tmp/project で 1 体だけ動いていることにする
        let fake_hc = work.join("fake-hc.sh");
        std::fs::write(
            &fake_hc,
            r#"#!/bin/sh
echo '{"terminals":[{"cwd":"/tmp/project","state":"live"},{"cwd":"/tmp/other","state":"finished"}]}'
"#,
        )
        .unwrap();
        make_executable(&fake_hc);

        // 動いていないプロジェクトのセッション（一覧に出てはいけない）
        let other = work.join("pi-sessions/--tmp-other--");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join(format!("2026-01-01T00-00-00-000Z_{OTHER_ID}.jsonl")),
            format!(
                r#"{{"type":"session","id":"{OTHER_ID}","cwd":"/tmp/other"}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"止まっているプロジェクトの頼みごと"}}]}}}}
"#
            ),
        )
        .unwrap();

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
            .env("OSD_SESSIONS_DIR", &sessions)
            .env("OSD_HC_BIN", &fake_hc)
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

    // セッション一覧: 最初のユーザー発言を添えて返す
    let (code, body) = s.call("GET", "/api/sessions", &s.token, "");
    assert_eq!(code, 200);
    assert!(body.contains("最初の頼みごと"), "一覧に見分けが付く文: {body}");
    assert!(body.contains("\"project\":\"project\""), "cwd から project: {body}");
    assert!(
        !body.contains("止まっているプロジェクト"),
        "動いていないプロジェクトのセッションは出さない: {body}"
    );
    assert!(body.contains("\"agents\":1"), "動いているエージェント数: {body}");

    // セッション内のアシスタント発言の一覧
    let file = s
        .work
        .join("pi-sessions/--tmp-project--")
        .join(format!("2026-01-01T00-00-00-000Z_{SESSION_ID}.jsonl"));
    let body_req = format!(r#"{{"file":"{}"}}"#, file.display());
    let (code, body) = s.call("POST", "/api/comments", &s.token, &body_req);
    assert_eq!(code, 200);
    assert!(body.contains("\"index\":1"), "2 件目まで数える: {body}");
    assert!(body.contains("古い答え"), "一覧に見分けが付く文: {body}");

    // 選んだセッションの最後のアシスタント発言を本筋にする
    let (code, body) = s.call("POST", "/api/open", &s.token, &body_req);
    assert_eq!(code, 200);
    assert!(body.contains("最後の答え"), "最後の発言を返す: {body}");
    assert_eq!(
        std::fs::read_to_string(s.work.join("main.md")).unwrap().trim(),
        "最後の答え"
    );

    // index 指定で途中の発言を選べる
    let body_req = format!(r#"{{"file":"{}","index":0}}"#, file.display());
    let (code, body) = s.call("POST", "/api/open", &s.token, &body_req);
    assert_eq!(code, 200);
    assert!(body.contains("古い答え"), "index で選んだ発言: {body}");

    // 置き場の外は読ませない
    let (_, body) = s.call(
        "POST",
        "/api/open",
        &s.token,
        r#"{"file":"/etc/passwd"}"#,
    );
    assert!(body.contains("ありません"), "置き場の外は断る: {body}");

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
    // /api/open で本筋にしたセッションから分岐する（サーバは実パスに直して覚える）
    let canonical = std::fs::canonicalize(&file).unwrap();
    assert!(
        argv.contains(&format!("--fork\n{}", canonical.display())),
        "選んだセッションから分岐する: {argv}"
    );
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
