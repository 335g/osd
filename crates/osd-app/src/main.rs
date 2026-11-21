//! osd の GUI。ブリッジ API（osd-server）を叩いて、本筋と付箋を描く。
//!
//! 使い方: osd-app [--url http://127.0.0.1:8765/?t=...] [--dir <作業フォルダ>]
//! `--url` も `--dir` も無ければ `OSD_URL`、次にカレントの `url.txt` を読む。

use osd_api::{Action, AskReq, AskResp, InitResp};
use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, ScrollSpec, TextEdit, Tx, Waker, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Markdown, TextArea, root};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, channel};

fn main() {
    rune::run(AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        scenes: &[("note", sample_note), ("ask", sample_ask)],
        ..AppSpec::new("osd", (1100.0, 760.0), init, view)
    });
}

// ---------- 状態 ----------
struct Note {
    id: u32,
    anchor: String,
    label: &'static str,
    answer: String,
    fork_file: String,
    pending: bool,
    err: String,
    follow: TextEdit,
}

struct St {
    bridge: Option<(String, String)>, // (origin, token)
    project: String,
    blocks: Vec<String>,
    selected: Option<usize>,
    notes: Vec<Note>,
    next_note: u32,
    question: TextEdit,
    ret: TextEdit,
    status: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    waker: Option<Waker>,
    busy: usize,
}

enum Msg {
    Init(Result<InitResp, String>),
    Ask(u32, Result<AskResp, String>),
    Draft(Result<AskResp, String>),
    Closed(Result<(), String>),
}

impl St {
    fn send(&self, job: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, waker) = (self.tx.clone(), self.waker.clone());
        std::thread::spawn(move || {
            let msg = job();
            let _ = tx.send(msg);
            if let Some(wake) = waker {
                wake();
            }
        });
    }

    fn note(&self, id: u32) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }
}

fn init() -> St {
    let url = bridge_url();
    let (tx, rx) = channel();
    St {
        bridge: url,
        project: String::new(),
        blocks: Vec::new(),
        selected: None,
        notes: Vec::new(),
        next_note: 1,
        question: TextEdit::new(""),
        ret: TextEdit::new(""),
        status: String::new(),
        tx,
        rx,
        waker: None,
        busy: 0,
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

/// ヘッドレスで付箋の見た目を確認するための見本（ブリッジには繋がない）。
fn sample_note(s: &mut St) {
    s.bridge = None;
    s.project = "見本".into();
    s.blocks = split_blocks(
        "## 恒等写像とは\n\n`fn id<T>(x: T) -> T { x }` は入力をそのまま返す関数です。\n\n```mermaid\nflowchart LR\n  A[\"入力 x\"] --> B[\"id\"] --> C[\"出力 x\"]\n```",
    );
    s.selected = Some(0);
    s.notes.push(Note {
        id: 1,
        anchor: "`fn id<T>(x: T) -> T { x }` は入力をそのまま返す関数です。".into(),
        label: "図",
        answer: "処理の流れは一直線です。受け取った値をそのまま返すので、途中に変換も副作用もありません。\n\n```mermaid\nflowchart LR\n  A[\"入力\"] --> B[\"id\"] --> C[\"出力\"]\n```".into(),
        fork_file: String::new(),
        pending: false,
        err: String::new(),
        follow: TextEdit::new(""),
    });
}

/// ヘッドレスで往復を確認するための見本（1 問だけ投げる）。
fn sample_ask(s: &mut St) {
    s.project = "見本".into();
    s.blocks = split_blocks("## 恒等写像とは\n\n`fn id<T>(x: T) -> T { x }` は入力をそのまま返す関数です。");
    if s.bridge.is_some() {
        start_note(s, Action::Detail, s.blocks[1].clone(), String::new());
    }
}

fn setup(s: &mut St, waker: Waker) {
    s.waker = Some(waker);
    let Some((origin, token)) = s.bridge.clone() else {
        s.status = "ブリッジの URL がありません（--url か --dir で渡してください）".into();
        return;
    };
    s.busy += 1;
    s.send(move || Msg::Init(http(&origin, &token, "GET", "/api/init", None).and_then(|b| {
        serde_json::from_str::<InitResp>(&b).map_err(|e| format!("応答を読めません: {e}"))
    })));
}

fn poll(s: &mut St, _t: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    while let Ok(msg) = s.rx.try_recv() {
        changed = true;
        match msg {
            Msg::Init(Ok(init)) => {
                s.project = if init.title.is_empty() { init.project } else { init.title };
                s.blocks = split_blocks(&init.markdown);
                s.selected = None;
                s.busy = s.busy.saturating_sub(1);
            }
            Msg::Init(Err(e)) => {
                s.status = e;
                s.busy = s.busy.saturating_sub(1);
            }
            Msg::Ask(id, res) => {
                s.busy = s.busy.saturating_sub(1);
                if let Some(n) = s.notes.iter_mut().find(|n| n.id == id) {
                    n.pending = false;
                    match res {
                        Ok(a) if a.ok => {
                            n.answer = a.text;
                            n.fork_file = a.fork_file;
                            n.err.clear();
                        }
                        Ok(a) => n.err = a.error,
                        Err(e) => n.err = e,
                    }
                }
            }
            Msg::Draft(res) => {
                s.busy = s.busy.saturating_sub(1);
                match res {
                    Ok(a) if a.ok => s.ret = TextEdit::new(a.text.trim()),
                    Ok(a) => s.status = a.error,
                    Err(e) => s.status = e,
                }
            }
            Msg::Closed(res) => {
                s.busy = s.busy.saturating_sub(1);
                match res {
                    Ok(()) => tx.quit(),
                    Err(e) => s.status = e,
                }
            }
        }
    }
    // 待っている間は時計を進めて、画面が固まって見えないようにする
    if s.busy > 0 {
        tx.wake_after(std::time::Duration::from_millis(200));
        changed = true;
    }
    changed
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
            text(if s.busy > 0 { "考えています…" } else { s.status.as_str() })
                .font(type_::LABEL)
                .color(if s.status.is_empty() { t.text_secondary } else { t.text }),
        );

    let body = row()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(1.0)
        .fill(t.separator)
        .child(main_pane(s, &k, t))
        .child(side_pane(s, &k, t));

    root(col().w_full().h_full().fill(t.bg).child(header).child(body), vec![])
}

fn main_pane(s: &St, k: &Kit, t: tok::Theme) -> El {
    let mut list = col().w_full().gap(2.0).padding(12.0);
    for (i, block) in s.blocks.iter().enumerate() {
        let chosen = s.selected == Some(i);
        list = list.child(
            el()
                .key(format!("blk-{i}"))
                .w_full()
                .padding(8.0)
                .radius(6.0)
                .fill(if chosen { t.card } else { t.bg })
                .on_press(move |s: &mut St| s.selected = Some(i))
                .child(Markdown::new(block.clone()).selectable(true).build(k)),
        );
    }
    if s.blocks.is_empty() && s.busy == 0 {
        list = list.child(text("本筋がありません").font(type_::BODY).color(t.text_secondary));
    }
    col()
        .key("main")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .min_w(0.0)
        .fill(t.bg)
        .child(list.scroll(ScrollSpec { stick_bottom: false }))
}

fn side_pane(s: &St, k: &Kit, t: tok::Theme) -> El {
    let mut panel = col().w_full().gap(10.0).padding(12.0);

    if let Some(i) = s.selected {
        let anchor = s.blocks.get(i).cloned().unwrap_or_default();
        panel = panel.child(composer(s, k, t, anchor));
    }

    for note in &s.notes {
        panel = panel.child(note_card(note, k, t));
    }

    panel = panel.child(
        col()
            .w_full()
            .gap(6.0)
            .padding(10.0)
            .radius(8.0)
            .fill(t.card)
            .child(text("本筋に返す文").font(type_::LABEL).color(t.text))
            .child(
                TextArea::new(&s.ret)
                    .placeholder("理解したことと、本筋に反映してほしい点")
                    .rows(3)
                    .on_edit(|s: &mut St| &mut s.ret)
                    .build(k),
            )
            .child(
                row().gap(8.0).child(
                    Button::new("下書きを作る")
                        .variant(ButtonVariant::Neutral)
                        .on_press(|s: &mut St| draft(s))
                        .build(k),
                ),
            )
            .child(
                Button::new("返して閉じる")
                    .variant(ButtonVariant::Contrast)
                    .on_press(|s: &mut St| close(s))
                    .build(k),
            ),
    );

    col()
        .key("side")
        .w(420.0)
        .h_full()
        .min_h(0.0)
        .fill(t.bg)
        .child(panel.scroll(ScrollSpec { stick_bottom: false }))
}

fn composer(s: &St, k: &Kit, t: tok::Theme, anchor: String) -> El {
    let plain = plain_text(&anchor);
    let preview = if plain.chars().count() > 160 {
        plain.chars().take(160).collect::<String>() + "…"
    } else {
        plain
    };
    let mut actions = row().gap(6.0);
    for (label, action) in [
        ("詳しく", Action::Detail),
        ("用語", Action::Term),
        ("図", Action::Diagram),
        ("レビュー", Action::Review),
    ] {
        let anchor = anchor.clone();
        actions = actions.child(
            Button::new(label)
                .variant(ButtonVariant::Neutral)
                .on_press(move |s: &mut St| {
                    start_note(s, action, anchor.clone(), String::new());
                    s.selected = None;
                })
                .build(k),
        );
    }
    let anchor_for_send = anchor.clone();

    col()
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(8.0)
        .fill(t.card)
        .child(text(preview).font(type_::LABEL).color(t.text_secondary))
        .child(actions)
        .child(
            TextArea::new(&s.question)
                .placeholder("この部分について質問する")
                .rows(2)
                .on_edit(|s: &mut St| &mut s.question)
                .build(k),
        )
        .child(
            Button::new("質問する")
                .variant(ButtonVariant::Contrast)
                .on_press(move |s: &mut St| {
                    let q = s.question.text.trim().to_owned();
                    start_note(s, Action::Question, anchor_for_send.clone(), q);
                    s.question = TextEdit::new("");
                    s.selected = None;
                })
                .build(k),
        )
}

fn note_card(note: &Note, k: &Kit, t: tok::Theme) -> El {
    let id = note.id;
    let mut card = col()
        .key(format!("note-{id}"))
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(8.0)
        .fill(t.card)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .child(text(note.label).font(type_::LABEL).color(tok::accent(0, true)))
                .child(text("").grow(1.0)),
        );

    if note.pending {
        card = card.child(text("考えています…").font(type_::BODY).color(t.text_secondary));
    }
    if !note.err.is_empty() {
        card = card.child(text(note.err.clone()).font(type_::BODY).color(t.text));
    }
    if !note.answer.is_empty() {
        card = card.child(Markdown::new(note.answer.clone()).selectable(true).build(k));
    }

    card.child(
        row()
            .w_full()
            .gap(6.0)
            .child(
                TextArea::new(&note.follow)
                    .placeholder("続けて聞く")
                    .rows(1)
                    .on_edit(move |s: &mut St| &mut s.notes.iter_mut().find(|n| n.id == id).unwrap().follow)
                    .build(k)
                    .grow(1.0),
            )
            .child(
                Button::new("送る")
                    .variant(ButtonVariant::Neutral)
                    .on_press(move |s: &mut St| follow_up(s, id))
                    .build(k),
            ),
    )
}

// ---------- 操作 ----------
fn start_note(s: &mut St, action: Action, anchor: String, question: String) {
    let id = s.next_note;
    s.next_note += 1;
    s.notes.push(Note {
        id,
        anchor: anchor.clone(),
        label: label_of(action),
        answer: String::new(),
        fork_file: String::new(),
        pending: true,
        err: String::new(),
        follow: TextEdit::new(""),
    });
    let Some((origin, token)) = s.bridge.clone() else {
        if let Some(n) = s.notes.iter_mut().find(|n| n.id == id) {
            n.pending = false;
            n.err = "ブリッジに繋がっていません".into();
        }
        return;
    };
    let body = serde_json::to_string(&AskReq {
        action,
        anchor,
        question,
        ..AskReq::default()
    })
    .unwrap_or_default();
    s.busy += 1;
    s.send(move || {
        Msg::Ask(
            id,
            http(&origin, &token, "POST", "/api/ask", Some(&body))
                .and_then(|b| serde_json::from_str::<AskResp>(&b).map_err(|e| e.to_string())),
        )
    });
}

fn follow_up(s: &mut St, id: u32) {
    let Some(note) = s.note(id) else { return };
    let (text, fork_file) = (note.follow.text.trim().to_owned(), note.fork_file.clone());
    if text.is_empty() {
        return;
    }
    let Some((origin, token)) = s.bridge.clone() else { return };
    let body = serde_json::to_string(&AskReq {
        follow_up: text,
        continue_session: !fork_file.is_empty(),
        fork_file,
        ..AskReq::default()
    })
    .unwrap_or_default();
    if let Some(n) = s.notes.iter_mut().find(|n| n.id == id) {
        n.pending = true;
        n.follow = TextEdit::new("");
    }
    s.busy += 1;
    s.send(move || {
        Msg::Ask(
            id,
            http(&origin, &token, "POST", "/api/ask", Some(&body))
                .and_then(|b| serde_json::from_str::<AskResp>(&b).map_err(|e| e.to_string())),
        )
    });
}

fn draft(s: &mut St) {
    let Some((origin, token)) = s.bridge.clone() else { return };
    let mut notes = String::new();
    for n in &s.notes {
        if !n.answer.is_empty() {
            notes.push_str(&format!("## {}\n{}\n\n", n.anchor, n.answer));
        }
    }
    if notes.trim().is_empty() {
        s.status = "先に付箋で理解を進めてください".into();
        return;
    }
    let body = serde_json::to_string(&serde_json::json!({ "notes": notes })).unwrap_or_default();
    s.busy += 1;
    s.send(move || {
        Msg::Draft(
            http(&origin, &token, "POST", "/api/draft", Some(&body))
                .and_then(|b| serde_json::from_str::<AskResp>(&b).map_err(|e| e.to_string())),
        )
    });
}

fn close(s: &mut St) {
    let Some((origin, token)) = s.bridge.clone() else { return };
    let body =
        serde_json::to_string(&serde_json::json!({ "text": s.ret.text })).unwrap_or_default();
    s.busy += 1;
    s.send(move || {
        Msg::Closed(
            http(&origin, &token, "POST", "/api/return", Some(&body)).map(|_| ()),
        )
    });
}

/// 付箋に貼る見出し用に、記号を落として1行にまとめる。
fn plain_text(md: &str) -> String {
    md.lines()
        .map(|l| l.trim_start_matches(['#', '>', '-', ' ']).replace(['`', '*'], ""))
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn label_of(action: Action) -> &'static str {
    match action {
        Action::Question => "質問",
        Action::Detail => "詳しく",
        Action::Term => "用語",
        Action::Diagram => "図",
        Action::Review => "レビュー",
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

// ---------- 本筋を付箋の単位に切る ----------
/// 空行で区切る。ただしコードフェンスの中は切らない（mermaid やコードを割らないため）。
fn split_blocks(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut fence = false;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
        }
        if line.trim().is_empty() && !fence {
            if !cur.trim().is_empty() {
                out.push(cur.trim_end().to_owned());
                cur.clear();
            }
            continue;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim_end().to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_break_on_blank_lines() {
        let md = "一段落目。\n続き。\n\n二段落目。\n";
        assert_eq!(split_blocks(md), vec!["一段落目。\n続き。", "二段落目。"]);
    }

    #[test]
    fn blocks_keep_fences_whole() {
        let md = "説明。\n\n```mermaid\nflowchart LR\n  A --> B\n\n  B --> C\n```\n\n最後。";
        let blocks = split_blocks(md);
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert!(blocks[1].starts_with("```mermaid"), "{blocks:?}");
        assert!(blocks[1].ends_with("```"), "{blocks:?}");
    }

    #[test]
    fn http_talks_to_a_local_server() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
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
            let mut stream = stream;
            let out = r#"{"ok":true}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                out.len(),
                out
            )
            .unwrap();
        });

        let body = r#"{"text":"x"}"#;
        let got = http(&format!("http://{addr}"), "abc", "POST", "/api/return", Some(body)).unwrap();
        assert_eq!(got, r#"{"ok":true}"#);
        server.join().unwrap();
    }
}
