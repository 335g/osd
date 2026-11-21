//! セッションファイル（`~/.pi/agent/sessions/<slug>/<時刻>_<id>.jsonl`）を読む。

use osd_api::SessionInfo;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// セッションの置き場。OSD_SESSIONS_DIR で差し替えられる（テスト用）。
pub fn dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OSD_SESSIONS_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".pi/agent/sessions")
}

/// 全セッションを新しい順に返す。タイトルが分からないので、最初のユーザー発言を添える。
pub fn list(base: &Path) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let Ok(slugs) = std::fs::read_dir(base) else {
        return out;
    };
    for slug in slugs.flatten() {
        let Ok(files) = std::fs::read_dir(slug.path()) else {
            continue;
        };
        for entry in files.flatten() {
            let file = entry.path();
            if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(summary) = scan(&file) else { continue };
            let updated = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            out.push(SessionInfo {
                id: summary.id,
                file: file.to_string_lossy().into_owned(),
                project: Path::new(&summary.cwd)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                cwd: summary.cwd,
                updated,
                preview: summary.preview,
            });
        }
    }
    out.sort_by(|a, b| b.updated.cmp(&a.updated));
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

/// そのセッションが `base` の中にあるかを確かめたうえで、パスを返す。
pub fn resolve(base: &Path, file: &str) -> Option<PathBuf> {
    let path = std::fs::canonicalize(file).ok()?;
    let base = std::fs::canonicalize(base).ok()?;
    path.starts_with(&base).then_some(path)
}

/// そのセッションが動いていた cwd（無ければ空文字）。
pub fn cwd_of(file: &Path) -> String {
    scan(file).map(|s| s.cwd).unwrap_or_default()
}

struct Summary {
    id: String,
    cwd: String,
    preview: String,
}

/// 一覧に出す分だけ読む。冒頭の数行に ID・cwd・最初のユーザー発言がそろっている。
fn scan(file: &Path) -> Option<Summary> {
    let reader = BufReader::new(std::fs::File::open(file).ok()?.take(512 * 1024));
    let (mut id, mut cwd, mut preview) = (None, String::new(), String::new());
    for line in reader.lines().take(40) {
        let Ok(line) = line else { break };
        // 巨大なシステムプロンプトの行は、探しているものが入っていないので解析しない
        if line.contains("\"role\":\"system\"") {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match ev["type"].as_str() {
            Some("session") => {
                id = ev["id"].as_str().map(str::to_owned);
                cwd = ev["cwd"].as_str().unwrap_or_default().to_owned();
            }
            Some("message") if ev["message"]["role"] == "user" => {
                preview = one_line(&assistant_text(&ev["message"]), 120);
                break;
            }
            _ => {}
        }
    }
    Some(Summary {
        id: id?,
        cwd,
        preview,
    })
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

fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(max).collect()
}
