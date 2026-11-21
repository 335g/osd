//! pi をフォークして質問に答えさせる部分。本体セッションには一切書き込まない。

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// 説明役にファイルを触らせない。読む道具（read/grep/find/ls）は review のために残す。
const EXCLUDE_TOOLS: &str = "bash,edit,write,codemode,subagent,intercom";

pub const SYSTEM_APPEND: &str = "\
あなたは今「understand-mode」の説明役です。ユーザーは、この会話でAIが出した内容（本筋）を理解しようとしています。
目的は説明して理解を助けることです。ファイルの編集はしないでください。
日本語で、短く、具体例を使って答えてください。見出しは使わず、段落と短い箇条書きだけにしてください。
「何を読んだか」「これから調べます」のような作業の説明は書かず、答えから書き始めてください。
図があると分かりやすい場合は ```mermaid のコードブロックで図を添えてください（flowchart か sequenceDiagram を推奨。ラベルは日本語可、記号は避ける）。
説明にコードが必要なときは、必要な部分だけ（15行以内）を載せてください。実際のファイルを読んでから写すこと（記憶で書かない）。
コードを載せるときの見出し行は「~~~~言語名 ファイルパス:開始行番号」にしてください（例：~~~~python src/accuracy.py:12）。
本筋の記述に誤りや見直すべき点を見つけたら、最後に一文で「本筋への影響：…」と書いてください。";

pub struct PiRequest<'a> {
    pub bin: &'a str,
    pub cwd: &'a Path,
    pub fork_dir: &'a Path,
    /// 分岐元（本体セッションか親の付箋）。`session` と排他。
    pub fork: Option<&'a str>,
    /// 積む先（同じ付箋への続きの質問）。
    pub session: Option<&'a str>,
    pub prompt: &'a str,
    pub timeout: Duration,
}

#[derive(Default)]
pub struct PiOut {
    pub text: String,
    pub session_id: Option<String>,
    pub fork_file: Option<String>,
    pub cost_usd: f64,
}

pub async fn run(req: &PiRequest<'_>) -> Result<PiOut, String> {
    let mut cmd = Command::new(req.bin);
    cmd.arg("-p")
        .arg("--mode")
        .arg("json")
        .arg("--session-dir")
        .arg(req.fork_dir)
        .arg("--exclude-tools")
        .arg(EXCLUDE_TOOLS)
        .arg("--append-system-prompt")
        .arg(SYSTEM_APPEND);
    match (req.fork, req.session) {
        (Some(f), _) => {
            cmd.arg("--fork").arg(f);
        }
        (None, Some(s)) => {
            cmd.arg("--session").arg(s);
        }
        (None, None) => return Err("分岐元も積む先も指定されていません".into()),
    }
    cmd.arg("--").arg(req.prompt);

    cmd.current_dir(req.cwd)
        // 開けたままだと pi が入力を待つので閉じる
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 時間切れで future を捨てたら子プロセスも死ぬようにする
        .kill_on_drop(true);

    let out = match tokio::time::timeout(req.timeout, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("piを起動できませんでした: {e}")),
        Err(_) => {
            return Err(format!(
                "{}秒たってもpiが答えませんでした",
                req.timeout.as_secs()
            ))
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut parsed = PiOut::default();
    for line in stdout.lines() {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if ev["type"] == "session" {
            parsed.session_id = ev["id"].as_str().map(str::to_owned);
        }
        if ev["type"] != "message_end" || ev["message"]["role"] != "assistant" {
            continue;
        }
        let parts: Vec<&str> = ev["message"]["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|c| c["type"] == "text")
                    .filter_map(|c| c["text"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        if !parts.is_empty() {
            parsed.text = parts.join("\n");
        }
        parsed.cost_usd += ev["message"]["usage"]["cost"]["total"].as_f64().unwrap_or(0.0);
    }

    if !out.status.success() && parsed.text.is_empty() {
        let tail: String = [stderr.as_ref(), stdout.as_ref()]
            .concat()
            .chars()
            .rev()
            .take(800)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!(
            "piが終了コード {} で終わりました\n{}",
            out.status.code().unwrap_or(-1),
            tail
        ));
    }

    if let Some(id) = &parsed.session_id {
        parsed.fork_file = fork_file_of(req.fork_dir, id).await;
    }
    Ok(parsed)
}

/// `--session-dir` に作られたフォークの実ファイルを、セッションIDから引く。
async fn fork_file_of(dir: &Path, id: &str) -> Option<String> {
    if id.len() != 36 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(&format!("_{id}.jsonl")) {
            return Some(join_display(dir, &name));
        }
    }
    None
}

fn join_display(dir: &Path, name: &str) -> String {
    PathBuf::from(dir).join(name).to_string_lossy().into_owned()
}
