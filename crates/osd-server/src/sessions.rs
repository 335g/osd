//! セッションファイル（`~/.pi/agent/sessions/<slug>/<時刻>_<id>.jsonl`）を読む。

use serde_json::Value;

use std::path::{Path, PathBuf};

/// セッションの置き場。OSD_SESSIONS_DIR で差し替えられる（テスト用）。
pub fn dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OSD_SESSIONS_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".pi/agent/sessions")
}

/// セッション ID（ファイル名の末尾 `_<id>.jsonl`）。
pub fn id_of(file: &Path) -> String {
    file.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .and_then(|stem| stem.rsplit_once('_').map(|(_, id)| id.to_owned()))
        .unwrap_or_default()
}

/// セッション ID（前方一致）からファイルを探す。
pub fn find(base: &Path, id: &str) -> Option<PathBuf> {
    let mut found = Vec::new();
    for slug in std::fs::read_dir(base).ok()?.flatten() {
        // 読めない項目（ファイルなど）があっても、そこで諦めない
        let Ok(files) = std::fs::read_dir(slug.path()) else {
            continue;
        };
        for entry in files.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let name = path.file_name()?.to_string_lossy().into_owned();
            if name.ends_with(&format!("_{id}.jsonl")) {
                // 完全一致を優先する
                found.insert(0, path);
            } else if found.is_empty() && name.contains(id) {
                found.push(path);
            }
        }
    }
    found.into_iter().next()
}

/// セッション内のアシスタント発言（テキストのあるものだけ、古い順）。
pub fn assistant_messages(file: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        // 巨大なシステムプロンプトの行は飛ばす
        if line.contains("\"role\":\"system\"") {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if ev["type"] != "message" || ev["message"]["role"] != "assistant" {
            continue;
        }
        let body = assistant_text(&ev["message"]);
        if !body.is_empty() {
            out.push(body);
        }
    }
    out
}

/// 選んだセッションの最後のアシスタント発言。無ければ None。
pub fn last_assistant(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut last = None;
    for line in text.lines() {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if ev["type"] != "message" || ev["message"]["role"] != "assistant" {
            continue;
        }
        let body = assistant_text(&ev["message"]);
        if !body.is_empty() {
            last = Some(body);
        }
    }
    last
}

/// そのセッションが動いていた cwd（無ければ空文字）。先頭の session 行だけ見る。
pub fn cwd_of(file: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(file) else {
        return String::new();
    };
    let Some(first) = text.lines().next() else {
        return String::new();
    };
    let Ok(ev) = serde_json::from_str::<Value>(first) else {
        return String::new();
    };
    ev["cwd"].as_str().unwrap_or_default().to_owned()
}

/// アシスタント（またはユーザー）発言の本文。テキストブロックだけを改行でつなぐ。
fn assistant_text(message: &Value) -> String {
    message["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|c| c["type"] == "text")
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 改行と連続空白を 1 つに潰して max 文字に切る。
pub fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(max).collect()
}
