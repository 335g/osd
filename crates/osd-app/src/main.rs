//! osd の GUI。ブリッジ API（osd-server）からセッション一覧を取り、選んだセッションの
//! 最後のアシスタント発言を「対象の AI コメント」として表示する。
//!
//! 使い方: osd-app [--url http://127.0.0.1:8765/?t=...] [--dir <作業フォルダ>]
//! `--url` も `--dir` も無ければ `OSD_URL`、次にカレントの `url.txt` を読む。

use osd_api::{CommentMeta, CommentsResp, InitResp, OpenReq, SessionInfo, SessionsResp};
use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, ScrollSpec, Tx, Waker, col, el, row, text};
use rune_kit::{Kit, Markdown, root};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, channel};

fn main() {
    rune::run(AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        scenes: &[("sample", sample)],
        ..AppSpec::new("osd", (1100.0, 760.0), init, view)
    });
}

// ---------- 状態 ----------
#[derive(Default)]
struct St {
    bridge: Option<(String, String)>, // (origin, token)
    sessions: Vec<SessionInfo>,
    /// いま開いているセッションのファイル
    opened: Option<String>,
    /// そのセッションのアシスタント発言（古い順）
    comments: Vec<CommentMeta>,
    /// 選んでいる発言の index
    picked: Option<u32>,
    project: String,
    markdown: String,
    status: String,
    tx: Option<Sender<Msg>>,
    rx: Option<Receiver<Msg>>,
    waker: Option<Waker>,
    busy: usize,
}

enum Msg {
    Sessions(Result<SessionsResp, String>),
    Comments(Result<CommentsResp, String>),
    Opened(u32, Result<InitResp, String>),
}

fn init() -> St {
    let (tx, rx) = channel();
    St {
        bridge: bridge_url(),
        tx: Some(tx),
        rx: Some(rx),
        ..St::default()
    }
}

/// 起動引数 → 環境変数 → カレントの url.txt の順にブリッジの URL を探す。
fn bridge_url() -> Option<(String, String)> {
    let args = rune::extra_args();
    let mut url = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--url" => url = args.get(i + 1).cloned(),
            "--dir" => {
                url = args
                    .get(i + 1)
                    .map(|d| format!("{d}/url.txt"))
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|s| s.trim().to_owned());
            }
            _ => {}
        }
        i += 1;
    }
    let url = url.or_else(|| std::env::var("OSD_URL").ok()).or_else(|| {
        std::fs::read_to_string("url.txt").ok().map(|s| s.trim().to_owned())
    })?;
    let (origin, token) = url.split_once("/?t=")?;
    Some((origin.to_owned(), token.to_owned()))
}

fn setup(s: &mut St, waker: Waker) {
    s.waker = Some(waker);
    if s.bridge.is_none() {
        s.status = "ブリッジの URL がありません（--url か --dir で渡してください）".into();
        return;
    }
    s.busy += 1;
    s.send(|origin, token| {
        Msg::Sessions(http(&origin, &token, "GET", "/api/sessions", None).and_then(|b| {
            serde_json::from_str::<SessionsResp>(&b).map_err(|e| format!("応答を読めません: {e}"))
        }))
    });
}

fn poll(s: &mut St, _t: f64, tx: &mut Tx) -> bool {
    let mut inbox = Vec::new();
    if let Some(rx) = &s.rx {
        while let Ok(msg) = rx.try_recv() {
            inbox.push(msg);
        }
    }
    let mut changed = !inbox.is_empty();
    for msg in inbox {
        s.busy = s.busy.saturating_sub(1);
        match msg {
            Msg::Sessions(Ok(list)) => {
                s.sessions = list.sessions;
                s.status.clear();
            }
            Msg::Sessions(Err(e)) => s.status = e,
            Msg::Comments(Ok(list)) => {
                s.project = list.project;
                // 既定は最後の発言（直前に AI が答えたもの）
                s.picked = list.comments.last().map(|c| c.index);
                s.comments = list.comments;
                if let Some(index) = s.picked {
                    if let Some(file) = s.opened.clone() {
                        open(s, file, index);
                    }
                }
            }
            Msg::Comments(Err(e)) => s.status = e,
            Msg::Opened(index, Ok(init)) => {
                s.project = init.project;
                s.markdown = init.markdown;
                s.picked = Some(index);
                s.status.clear();
            }
            Msg::Opened(_, Err(e)) => s.status = e,
        }
    }
    if s.busy > 0 {
        tx.wake_after(std::time::Duration::from_millis(200));
        changed = true;
    }
    changed
}

impl St {
    /// HTTP はスレッドで走らせ、終わったら channel で受けて waker で画面を起こす。
    fn send(&self, job: impl FnOnce(String, String) -> Msg + Send + 'static) {
        let (Some(bridge), Some(tx), waker) = (self.bridge.clone(), self.tx.clone(), self.waker.clone())
        else {
            return;
        };
        std::thread::spawn(move || {
            let _ = tx.send(job(bridge.0, bridge.1));
            if let Some(wake) = waker {
                wake();
            }
        });
    }

    /// セッションを開く: 発言の一覧を取り、最後のものを表示する。
    fn open_session(&mut self, file: String) {
        self.opened = Some(file.clone());
        self.comments.clear();
        self.picked = None;
        self.markdown.clear();
        self.busy += 1;
        let body = serde_json::to_string(&OpenReq {
            file,
            ..OpenReq::default()
        })
        .unwrap_or_default();
        self.send(move |origin, token| {
            Msg::Comments(
                http(&origin, &token, "POST", "/api/comments", Some(&body))
                    .and_then(|b| serde_json::from_str::<CommentsResp>(&b).map_err(|e| e.to_string())),
            )
        });
    }
}

/// 指定のアシスタント発言を本筋にして、その本文を表示する。
fn open(s: &mut St, file: String, index: u32) {
    s.busy += 1;
    let body = serde_json::to_string(&OpenReq {
        file,
        index: Some(index),
    })
    .unwrap_or_default();
    s.send(move |origin, token| {
        Msg::Opened(
            index,
            http(&origin, &token, "POST", "/api/open", Some(&body))
                .and_then(|b| serde_json::from_str::<InitResp>(&b).map_err(|e| e.to_string())),
        )
    });
}

// ---------- 見た目 ----------
fn kit() -> Kit {
    Kit::neutral(true)
}

fn view(s: &St, _: &mut Cx<St>) -> El {
    let t = tok::theme(true);
    let k = kit();
    let header = row()
        .w_full()
        .px(16.0)
        .py(10.0)
        .gap(12.0)
        .center_y()
        .fill(t.card)
        .child(text(format!("osd · {}", s.project)).font(type_::LABEL).color(t.text))
        .child(
            text(if s.busy > 0 { "読み込んでいます…" } else { s.status.as_str() })
                .font(type_::LABEL)
                .color(t.text_secondary),
        );

    let body = row()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(1.0)
        .fill(t.separator)
        .child(session_list(s, t))
        .child(comment_pane(s, &k, t));

    root(col().w_full().h_full().fill(t.bg).child(header).child(body), vec![])
}

fn session_list(s: &St, t: tok::Theme) -> El {
    let mut list = col().w_full().gap(2.0).padding(8.0);
    if s.sessions.is_empty() && s.busy == 0 {
        list = list.child(text("セッションがありません").font(type_::BODY).color(t.text_secondary));
    }
    for session in &s.sessions {
        let chosen = s.opened.as_deref() == Some(session.file.as_str());
        let file = session.file.clone();
        list = list.child(
            el()
                .key(format!("s-{}", session.id))
                .w_full()
                .padding(8.0)
                .radius(6.0)
                .fill(if chosen { t.hover } else { t.bg })
                .on_press(move |s: &mut St| s.open_session(file.clone()))
                .child(
                    col()
                        .w_full()
                        .gap(3.0)
                        .child(
                            row()
                                .w_full()
                                .gap(8.0)
                                .child(text(session.project.clone()).font(type_::LABEL).color(t.text))
                                .child(text(when(session.updated)).font(type_::LABEL).color(t.text_secondary)),
                        )
                        .child(
                            text(session.preview.clone())
                                .font(type_::BODY)
                                .wrap()
                                .max_lines(2)
                                .color(t.text_secondary),
                        ),
                ),
        );
    }
    col()
        .key("sessions")
        .w(380.0)
        .h_full()
        .min_h(0.0)
        .fill(t.bg)
        .child(list.scroll(ScrollSpec { stick_bottom: false }))
}

fn comment_pane(s: &St, k: &Kit, t: tok::Theme) -> El {
    col()
        .key("comment")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .min_w(0.0)
        .fill(t.bg)
        .child(comment_list(s, t))
        .child(comment_body(s, k, t))
}

/// 上: そのセッションのアシスタント発言の一覧。
fn comment_list(s: &St, t: tok::Theme) -> El {
    let mut list = col().w_full().gap(2.0).padding(8.0);
    if s.comments.is_empty() {
        list = list.child(
            text(if s.opened.is_some() {
                "このセッションにはアシスタントの発言がありません"
            } else {
                "左からセッションを選んでください"
            })
            .font(type_::BODY)
            .color(t.text_secondary),
        );
    }
    for comment in &s.comments {
        let chosen = s.picked == Some(comment.index);
        let index = comment.index;
        let file = s.opened.clone().unwrap_or_default();
        list = list.child(
            el()
                .key(format!("c-{index}"))
                .w_full()
                .padding(6.0)
                .radius(6.0)
                .fill(if chosen { t.hover } else { t.bg })
                .on_press(move |s: &mut St| open(s, file.clone(), index))
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .child(
                            text(format!("{index:02}"))
                                .font(type_::LABEL)
                                .color(if chosen { tok::accent(0, true) } else { t.text_secondary }),
                        )
                        .child(
                            text(comment.preview.clone())
                                .font(type_::BODY)
                                .wrap()
                                .grow(1.0)
                                .min_w(0.0)
                                .max_lines(2)
                                .color(if chosen { t.text } else { t.text_secondary }),
                        ),
                ),
        );
    }
    // 発言が多いセッションでも本文を圧迫しないよう、上は 220pt まで
    col()
        .key("comments")
        .w_full()
        .max_h(220.0)
        .min_h(0.0)
        .shrink()
        .fill(t.bg)
        .child(list.scroll(ScrollSpec { stick_bottom: false }))
}

/// 下: 選んだ発言の本文。
fn comment_body(s: &St, k: &Kit, t: tok::Theme) -> El {
    let body = if s.markdown.is_empty() {
        col().padding(16.0).child(
            text("上から発言を選ぶと、ここに出ます")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
    } else {
        col()
            .w_full()
            .padding(16.0)
            .child(Markdown::new(s.markdown.clone()).selectable(true).build(k))
    };
    col()
        .key("body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.bg)
        .child(body.scroll(ScrollSpec { stick_bottom: false }))
}

/// 更新時刻を「いま / N分前 / N時間前 / N日前」に。
fn when(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let ago = now.saturating_sub(ms) / 1000;
    match ago {
        0..=59 => "いま".to_string(),
        60..=3599 => format!("{}分前", ago / 60),
        3600..=86399 => format!("{}時間前", ago / 3600),
        _ => format!("{}日前", ago / 86400),
    }
}

// ---------- ブリッジの呼び出し ----------
/// localhost のブリッジ専用の最小 HTTP クライアント。
/// shortcut: チャンク転送と keep-alive は扱わない（osd-server は Content-Length で閉じる応答だけ返す）。
fn http(origin: &str, token: &str, method: &str, path: &str, body: Option<&str>) -> Result<String, String> {
    let addr = origin.trim_start_matches("http://");
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("{addr} に繋げません: {e}"))?;
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nX-Osd-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("送信できません: {e}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("受信できません: {e}"))?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "応答の形が想定外です".to_string())?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if status != 200 {
        return Err(format!("HTTP {status}: {body}"));
    }
    Ok(body.to_owned())
}

// ---------- 見た目を確かめるための見本 ----------
fn sample(s: &mut St) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    s.project = "osd".into();
    s.sessions = vec![
        SessionInfo {
            id: "01a1fd24".into(),
            file: "/tmp/a.jsonl".into(),
            cwd: "/Users/335g/Dev/osd".into(),
            project: "osd".into(),
            updated: now,
            preview: "APIサーバーはaxumですかね".into(),
        },
        SessionInfo {
            id: "01a1fcdd".into(),
            file: "/tmp/b.jsonl".into(),
            cwd: "/Users/335g/Dev/ghq/github.com/335g/tuit".into(),
            project: "tuit".into(),
            updated: now - 3_600_000,
            preview: "このテストが落ちる理由を調べて".into(),
        },
    ];
    s.opened = Some("/tmp/a.jsonl".into());
    s.comments = vec![
        CommentMeta { index: 0, preview: "まず恒等写像の定義を確認します。".into(), chars: 40 },
        CommentMeta { index: 1, preview: "`fn id<T>(x: T) -> T { x }` は入力をそのまま返す関数です。".into(), chars: 320 },
        CommentMeta { index: 2, preview: "図にすると一直線です。".into(), chars: 90 },
    ];
    s.picked = Some(1);
    s.markdown = "## 恒等写像とは\n\n`fn id<T>(x: T) -> T { x }` は入力をそのまま返す関数です。\n\n```mermaid\nflowchart LR\n  A[\"入力 x\"] --> B[\"id\"] --> C[\"出力 x\"]\n```\n\n| 用途 | 例 |\n|---|---|\n| 既定値 | `unwrap_or_else` |\n".into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn http_talks_to_a_local_server() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.strip_prefix("Content-Length: ") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&line);
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            assert!(head.contains("X-Osd-Token: abc"), "{head}");
            let out = r#"{"ok":true}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                out.len(),
                out
            )
            .unwrap();
        });

        let got = http(
            &format!("http://{addr}"),
            "abc",
            "POST",
            "/api/open",
            Some(r#"{"file":"/tmp/a.jsonl"}"#),
        )
        .unwrap();
        assert_eq!(got, r#"{"ok":true}"#);
        server.join().unwrap();
    }

    #[test]
    fn when_says_relative_time() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(when(now), "いま");
        assert_eq!(when(now - 120_000), "2分前");
        assert_eq!(when(now - 7_200_000), "2時間前");
    }
}
