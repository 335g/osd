//! いま動いているエージェントを herdr-core（`hc`）から取る。
//! `hc ls --json` の `terminals[]` のうち `state: "live"` の cwd を、動いているプロジェクトとみなす。

use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// (cwd, 名前, エージェント数) を cwd でまとめて返す。
pub async fn running() -> Result<Vec<(String, String, u32)>, String> {
    let bin = std::env::var("OSD_HC_BIN").unwrap_or_else(|_| "hc".into());
    let out = Command::new(&bin)
        .args(["ls", "--json"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("{bin} ls --json を実行できません: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{bin} ls --json が終了コード {} で終わりました",
            out.status.code().unwrap_or(-1)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(parse(&text))
}

fn parse(json: &str) -> Vec<(String, String, u32)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String, u32)> = Vec::new();
    for terminal in v["terminals"].as_array().into_iter().flatten() {
        if terminal["state"] != "live" {
            continue;
        }
        let Some(cwd) = terminal["cwd"].as_str().filter(|c| !c.is_empty()) else {
            continue;
        };
        match out.iter_mut().find(|(c, _, _)| c == cwd) {
            Some((_, _, n)) => *n += 1,
            None => {
                let name = Path::new(cwd)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| cwd.to_string());
                out.push((cwd.to_owned(), name, 1));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn keeps_live_projects_only() {
        let json = r#"{"terminals":[
            {"cwd":"/Users/me/Dev/osd","state":"live"},
            {"cwd":"/Users/me/Dev/osd","state":"live"},
            {"cwd":"/Users/me/Dev/tuit","state":"live"},
            {"cwd":"/Users/me/Dev/old","state":"finished"},
            {"cwd":"","state":"live"}
        ]}"#;
        let got = parse(json);
        assert_eq!(
            got,
            vec![
                ("/Users/me/Dev/osd".to_string(), "osd".to_string(), 2),
                ("/Users/me/Dev/tuit".to_string(), "tuit".to_string(), 1),
            ]
        );
    }

    #[test]
    fn broken_json_is_no_projects() {
        assert!(parse("not json").is_empty());
    }
}
