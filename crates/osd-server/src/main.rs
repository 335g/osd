//! osd ブリッジ API サーバ。1 つのセッション（プロジェクト）に絞って、
//! その中のアシスタント発言を「本筋」として扱う。付箋の質問は pi のフォークで答える。
//!
//! 使い方: osd-server --session <セッションIDかファイル> [--dir <作業フォルダ>] [--cwd <プロジェクト>] [--port 8765] [--host 127.0.0.1]
//!
//! 起動すると `osd: http://127.0.0.1:<port>/?t=<token>` を stdout に出し、同じ URL を
//! `<作業フォルダ>/url.txt` に書く。GUI はこの URL を叩く。

mod pi;
mod sessions;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use include_dir::{include_dir, Dir};
use osd_api::{
    Action, AskReq, AskResp, CommentMeta, CommentsResp, DraftReq, InitResp, OkResp, OpenReq,
    ReturnReq,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const DEFAULT_TIMEOUT_SEC: u64 = 300;

/// 開いているセッション。起動時に決まり、以後変わらない。
struct Session {
    file: PathBuf,
    id: String,
    project: String,
}

struct App {
    work_dir: PathBuf,
    fork_dir: PathBuf,
    project_cwd: PathBuf,
    session: Option<Session>,
    token: String,
    /// 同じマシンの別プロセス・別ページからの操作を断るための Host 許可リスト
    allowed_hosts: Vec<String>,
    timeout: Duration,
    shutdown: Notify,
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio ランタイムを作れませんでした");
    if let Err(e) = rt.block_on(run()) {
        eprintln!("osd-server: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1));
    let work_dir = match args.get("dir") {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir().join(format!("osd-{}", token(8))),
    };
    tokio::fs::create_dir_all(&work_dir)
        .await
        .map_err(|e| format!("{} を作れませんでした: {e}", work_dir.display()))?;

    // フォークは作業フォルダの中だけに作り、ここを丸ごと消せば後片付けが済む。
    // shortcut: 強制終了（SIGKILL）時は残るので、起動時に前回の残骸を消す。
    let fork_dir = work_dir.join("sessions");
    let _ = tokio::fs::remove_dir_all(&fork_dir).await;
    tokio::fs::create_dir_all(&fork_dir)
        .await
        .map_err(|e| format!("{} を作れませんでした: {e}", fork_dir.display()))?;

    let host = args.get("host").cloned().unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = args
        .get("port")
        .map(|p| p.parse().map_err(|_| format!("--port が数値ではありません: {p}")))
        .transpose()?
        .unwrap_or(8765);
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .map_err(|e| format!("{host}:{port} を listen できませんでした: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;

    let token = token(32);
    let mut allowed_hosts = vec![
        format!("127.0.0.1:{}", addr.port()),
        format!("localhost:{}", addr.port()),
    ];
    // WSL など、外から届く前提で 0.0.0.0 に開く場合は Host の許可を bind 先に広げる
    if !host.starts_with("127.") && host != "localhost" && host != "::1" {
        allowed_hosts.push(format!("{host}:{}", addr.port()));
        allowed_hosts.push(format!("{addr}:"));
    }

    let timeout = std::env::var("OSD_TIMEOUT_SEC")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_TIMEOUT_SEC));

    let session = resolve_session(&args);
    if let Some(s) = &session {
        // 最初は最後のアシスタント発言を本筋にしておく
        if let Some(markdown) = sessions::last_assistant(&s.file) {
            let _ = tokio::fs::write(work_dir.join("main.md"), &markdown).await;
        }
        let _ = tokio::fs::write(work_dir.join("title.txt"), format!("{}\n", s.project)).await;
    }

    let app = Arc::new(App {
        work_dir: work_dir.clone(),
        fork_dir,
        project_cwd: match args.get("cwd") {
            Some(dir) => PathBuf::from(dir),
            None => session
                .as_ref()
                .map(|s| PathBuf::from(sessions::cwd_of(&s.file)))
                .filter(|p| p.is_dir())
                .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?),
        },
        session,
        token: token.clone(),
        allowed_hosts,
        timeout,
        shutdown: Notify::new(),
    });

    let url = format!("http://{addr}/?t={token}");
    write_handoff(&work_dir, &url, std::process::id()).await;
    println!("osd: {url}");
    println!("作業フォルダ: {}", work_dir.display());
    match &app.session {
        Some(s) => println!("セッション: {} ({})", s.project, s.file.display()),
        None => println!("セッション: (未指定)"),
    }
    println!("分身を動かす場所: {}", app.project_cwd.display());

    let router = routes(app.clone());
    let closing = app.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = closing.shutdown.notified() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            cleanup_forks(&closing).await;
        })
        .await
        .map_err(|e| e.to_string())
}

/// 白板 UI の静的ファイル。バイナリに埋め込む（cargo install しても動くように）。
static PUBLIC: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/public");

fn routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/init", get(init))
        .route("/api/comments", get(comments))
        .route("/api/open", post(open))
        .route("/api/ask", post(ask))
        .route("/api/draft", post(draft))
        .route("/api/return", post(close))
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .fallback(static_file)
        .with_state(app)
}

/// 合言葉なしで配る白板 UI。/api 以外はここに来る。
async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match PUBLIC.get_file(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, content_type(path))],
            file.contents(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

// ---------- 認証 ----------
/// 同じマシンの別のページ・別のプロセスからの操作を断る。
/// Host を見るのは DNS リバインディング対策。合言葉は URL にだけ載せる。
async fn auth(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !app.allowed_hosts.iter().any(|h| h == host) {
        return deny("ホスト名が違います");
    }
    let given = req
        .headers()
        .get("x-osd-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| query_param(req.uri().query().unwrap_or_default(), "t"));
    match given {
        Some(g) if constant_time_eq(g.as_bytes(), app.token.as_bytes()) => next.run(req).await,
        _ => deny("合言葉が違います"),
    }
}

fn deny(why: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(OkResp {
            ok: false,
            error: format!("{why}。osd の URL から開き直してください。"),
        }),
    )
        .into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

// ---------- ハンドラ ----------
async fn init(State(app): State<Arc<App>>) -> Json<InitResp> {
    let mut markdown = tokio::fs::read_to_string(app.work_dir.join("main.md"))
        .await
        .unwrap_or_else(|_| "# 本筋がありません\n\nセッションを開いてください。".into());
    let mut title = String::new();
    if let Some(s) = &app.session {
        title = s.project.clone();
        if markdown.contains("本筋がありません") {
            markdown = sessions::last_assistant(&s.file).unwrap_or(markdown);
        }
    }
    Json(InitResp {
        markdown,
        title,
        has_session: app.session.is_some(),
        project: app.session.as_ref().map(|s| s.project.clone()).unwrap_or_default(),
    })
}

/// 開いているセッションのアシスタント発言の一覧。
async fn comments(State(app): State<Arc<App>>) -> Json<CommentsResp> {
    let Some(session) = &app.session else {
        return Json(CommentsResp {
            note: "セッションが指定されていません（--session で渡してください）".into(),
            ..CommentsResp::default()
        });
    };
    let file = session.file.clone();
    let messages = tokio::task::spawn_blocking(move || sessions::assistant_messages(&file))
        .await
        .unwrap_or_default();
    Json(CommentsResp {
        project: session.project.clone(),
        session: session.id.clone(),
        comments: messages
            .iter()
            .enumerate()
            .map(|(i, text)| CommentMeta {
                index: i as u32,
                preview: sessions::one_line(text, 120),
                chars: text.chars().count() as u32,
            })
            .collect(),
        note: String::new(),
    })
}

/// 選んだアシスタント発言を本筋にする（index 省略なら最後）。
async fn open(State(app): State<Arc<App>>, Json(body): Json<OpenReq>) -> Json<InitResp> {
    let Some(session) = &app.session else {
        return Json(InitResp {
            markdown: "セッションが指定されていません（--session で渡してください）".into(),
            ..InitResp::default()
        });
    };
    let file = session.file.clone();
    let index = body.index;
    let text = tokio::task::spawn_blocking(move || match index {
        Some(i) => sessions::assistant_messages(&file).get(i as usize).cloned(),
        None => sessions::last_assistant(&file),
    })
    .await
    .ok()
    .flatten();
    let Some(markdown) = text else {
        return Json(InitResp {
            markdown: "このセッションにはアシスタントの発言がありません".into(),
            title: session.project.clone(),
            has_session: true,
            project: session.project.clone(),
        });
    };
    if let Err(e) = tokio::fs::write(app.work_dir.join("main.md"), &markdown).await {
        return Json(InitResp {
            markdown: format!("main.md に書けませんでした: {e}"),
            ..InitResp::default()
        });
    }
    Json(InitResp {
        markdown,
        title: session.project.clone(),
        has_session: true,
        project: session.project.clone(),
    })
}

async fn ask(State(app): State<Arc<App>>, Json(body): Json<AskReq>) -> Json<AskResp> {
    // 最初の質問は本体（か親の付箋）から分岐する。続きの質問はそのフォークに積む。
    let parent = if body.continue_session {
        Some(body.fork_file.clone()).filter(|s| !s.is_empty())
    } else {
        session_file(&app).or_else(|| Some(body.session_id.clone()).filter(|s| !s.is_empty()))
    };
    let Some(parent) = parent else {
        return Json(AskResp::err("本体のセッションが見つかりません。起動し直してください。"));
    };
    let prompt = if body.follow_up.trim().is_empty() {
        action_prompt(body.action, &body.anchor, &body.question)
    } else {
        body.follow_up.clone()
    };

    let req = pi::PiRequest {
        bin: &pi_bin(),
        cwd: &app.project_cwd,
        fork_dir: &app.fork_dir,
        fork: (!body.continue_session).then(|| parent.as_str()),
        session: Some(parent.as_str()),
        prompt: &prompt,
        timeout: app.timeout,
    };
    match pi::run(&req).await {
        Ok(out) => Json(AskResp {
            ok: true,
            text: out.text,
            session_id: out.session_id.unwrap_or_default(),
            fork_file: out.fork_file.unwrap_or_default(),
            cost_usd: (out.cost_usd * 1e6).round() / 1e6,
            error: String::new(),
        }),
        Err(e) => Json(AskResp::err(e)),
    }
}

async fn draft(State(app): State<Arc<App>>, Json(body): Json<DraftReq>) -> Json<AskResp> {
    let Some(session) = session_file(&app) else {
        return Json(AskResp::err("本体のセッションが見つかりません。"));
    };
    let prompt = [
        "以下は、ユーザーがAIの出力（本筋）を理解するために交わした、付箋での質問と回答の記録です。",
        "これを読んで、メインの作業セッションに送る「次のプロンプト」をユーザーの一人称で書いてください。",
        "内容は2段落：(1) やりとりで理解・確定したことの要約（箇条書きで最大5点）、(2) それを踏まえて次にやってもらいたいこと（具体的に1〜3点）。",
        "前置きや説明は書かず、プロンプト本文だけを出力すること。",
        "",
        &body.notes,
    ]
    .join("\n");

    let req = pi::PiRequest {
        bin: &pi_bin(),
        cwd: &app.project_cwd,
        fork_dir: &app.fork_dir,
        fork: Some(&session),
        session: None,
        prompt: &prompt,
        timeout: app.timeout,
    };
    match pi::run(&req).await {
        Ok(out) => Json(AskResp {
            ok: true,
            text: out.text,
            session_id: out.session_id.unwrap_or_default(),
            fork_file: out.fork_file.unwrap_or_default(),
            cost_usd: (out.cost_usd * 1e6).round() / 1e6,
            error: String::new(),
        }),
        Err(e) => Json(AskResp::err(e)),
    }
}

async fn close(State(app): State<Arc<App>>, Json(body): Json<ReturnReq>) -> Json<OkResp> {
    let text = body.text.trim();
    let line = if text.is_empty() {
        "(本筋に返すことはありません)\n"
    } else {
        text
    };
    let resp = match std::fs::write(app.work_dir.join("return.md"), format!("{line}\n")) {
        Ok(()) => OkResp {
            ok: true,
            error: String::new(),
        },
        Err(e) => OkResp {
            ok: false,
            error: format!("return.md を書けませんでした: {e}"),
        },
    };
    cleanup_forks(&app).await;
    // 応答を返してから終わる（axum の graceful shutdown が処理中の応答を待つ）
    app.shutdown.notify_waiters();
    Json(resp)
}

// ---------- セッションの解決 ----------
/// `--session` にファイルか ID（前方一致）を渡せる。無ければ PI_SESSION_FILE。
fn resolve_session(args: &std::collections::HashMap<String, String>) -> Option<Session> {
    let given = args
        .get("session")
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| std::env::var("PI_SESSION_FILE").ok().filter(|s| !s.is_empty()))?;
    let path = PathBuf::from(&given);
    let file = if path.is_file() {
        std::fs::canonicalize(&path).ok()?
    } else {
        sessions::find(&sessions::dir(), &given)?
    };
    let id = sessions::id_of(&file);
    let project = Path::new(&sessions::cwd_of(&file))
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(Session { file, id, project })
}

fn session_file(app: &Arc<App>) -> Option<String> {
    app.session
        .as_ref()
        .map(|s| s.file.to_string_lossy().into_owned())
}

// ---------- 付箋のプロンプト ----------
fn action_prompt(action: Action, anchor: &str, question: &str) -> String {
    let target = format!("\n\n【対象】\n{anchor}");
    match action {
        Action::Question => {
            let q = if question.trim().is_empty() {
                "これはどういう意味ですか？"
            } else {
                question
            };
            format!("本筋のこの部分について質問です。{target}\n\n【質問】\n{q}")
        }
        Action::Detail => format!(
            "本筋のこの部分を、もう一段詳しく、具体例を使って説明してください。コードなら、何をしているかを処理のまとまりごとに説明してください。{target}"
        ),
        Action::Term => format!(
            "この用語（または表現・関数名）を、この会話の文脈での意味に絞って説明してください。一般的な定義と、ここでの使われ方の両方を短く。{target}"
        ),
        Action::Diagram => format!(
            "この部分を図で説明してください。必ず ```mermaid の図を1つ含め、図の読み方を2〜3文で添えてください。コードなら処理の流れかデータの流れを図にしてください。{target}"
        ),
        Action::Review => [
            "この部分をレビューしてください。必要ならリポジトリのファイルを読んで前後の文脈も確認してください（編集はしないこと）。",
            "観点：不具合、境界値（0・空・欠損・重複・型）、前提の誤り、読みやすさ。",
            "重要な順に最大3点。各点は「重大度（高/中/低） 何が起きるか（具体的な入力例つき）→ 直し方」を2〜3文で。",
            "問題がなければ「問題なし」と、その理由を一文で書いてください。",
            "直すべき点があれば、最後に一文で「本筋への影響：…」と書いてください。",
        ]
        .join("\n")
            + &target,
    }
}

// ---------- 後片付けと起動の小物 ----------
async fn cleanup_forks(app: &Arc<App>) {
    let _ = tokio::fs::remove_dir_all(&app.fork_dir).await;
}

async fn write_handoff(work_dir: &Path, url: &str, pid: u32) {
    let _ = tokio::fs::write(work_dir.join("url.txt"), format!("{url}\n")).await;
    let _ = tokio::fs::write(work_dir.join("server.pid"), pid.to_string()).await;
}

fn pi_bin() -> String {
    std::env::var("OSD_PI_BIN").unwrap_or_else(|_| "pi".into())
}

fn token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    // 乱数が取れない環境では起動を止める（推測できる合言葉で開けてしまうより良い）
    getrandom::fill(&mut buf).expect("乱数を取れませんでした");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_args(argv: impl Iterator<Item = String>) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut argv = argv.peekable();
    while let Some(a) = argv.next() {
        let Some(key) = a.strip_prefix("--") else {
            continue;
        };
        match argv.peek() {
            Some(next) if !next.starts_with("--") => {
                out.insert(key.to_string(), argv.next().unwrap());
            }
            _ => {
                out.insert(key.to_string(), String::new());
            }
        }
    }
    out
}
