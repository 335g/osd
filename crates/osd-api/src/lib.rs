//! ブリッジ API の型。サーバ（osd-server）と GUI（osd-app）で同じ定義を使う。

use serde::{Deserialize, Serialize};

/// 付箋のボタン。v1 は data（SQL）と Vega-Lite を持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Question,
    Detail,
    Term,
    Diagram,
    Review,
}

impl Default for Action {
    fn default() -> Self {
        Action::Question
    }
}

/// GET /api/init
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitResp {
    /// 本筋（直前のアシスタント回答）の markdown
    pub markdown: String,
    pub title: String,
    pub has_session: bool,
    pub project: String,
}

/// POST /api/ask
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AskReq {
    pub action: Action,
    /// 選んだ文・行
    pub anchor: String,
    pub question: String,
    /// 同じ付箋への続きの質問（指定すると action/anchor は使わない）
    pub follow_up: String,
    /// true なら fork_file に積む。false なら本体から分岐する
    pub continue_session: bool,
    /// 分岐した付箋の実ファイル。続きの質問で返ってきたものを渡す
    pub fork_file: String,
    /// 本体のセッション（起動引数を上書きしたいときだけ）
    pub session_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AskResp {
    pub ok: bool,
    pub text: String,
    pub session_id: String,
    /// この付箋のフォークの実ファイル。続きの質問ではこれを continueSession と一緒に返す
    pub fork_file: String,
    pub cost_usd: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

impl AskResp {
    pub fn err(msg: impl Into<String>) -> Self {
        AskResp {
            ok: false,
            text: String::new(),
            session_id: String::new(),
            fork_file: String::new(),
            cost_usd: 0.0,
            error: msg.into(),
        }
    }
}

/// POST /api/draft — 「本筋に返す文」の下書き
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftReq {
    pub notes: String,
}

/// POST /api/return — 白板を閉じ、返す文を return.md に書く
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReturnReq {
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OkResp {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// リストに出す 1 セッション分の情報。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionInfo {
    /// セッション ID（ファイル名の末尾 `_<id>.jsonl`）
    pub id: String,
    /// セッションファイルの絶対パス。`/api/open` に渡す
    pub file: String,
    pub cwd: String,
    /// cwd の最後のディレクトリ名
    pub project: String,
    /// 更新時刻（Unix ミリ秒）
    pub updated: u64,
    /// 最初のユーザー発言（何を頼んだセッションか分かるように）
    pub preview: String,
}

/// GET /api/sessions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionsResp {
    pub sessions: Vec<SessionInfo>,
}

/// POST /api/open — 選んだアシスタント発言を本筋にする（index 省略なら最後）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenReq {
    pub file: String,
    /// セッション内の何番目のアシスタント発言か（0 始まり）
    pub index: Option<u32>,
}

/// セッション内の 1 つのアシスタント発言（一覧に出す分だけ）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentMeta {
    pub index: u32,
    /// 一覧で見分けるための先頭部分（1 行）
    pub preview: String,
    /// 元の文字数
    pub chars: u32,
}

/// POST /api/comments
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentsResp {
    pub project: String,
    pub comments: Vec<CommentMeta>,
}
