use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// The configured human owner (`default_owner`), set once when a store loads.
static HUMAN_OWNER: OnceLock<String> = OnceLock::new();

/// Register the configured human owner so session helpers never treat them as an agent CLI.
pub fn set_human_owner(name: &str) {
    let _ = HUMAN_OWNER.set(name.trim().trim_start_matches('@').to_lowercase());
}

/// True for the configured owner and the generic human labels.
pub fn is_human(name: &str) -> bool {
    let n = name.trim().trim_start_matches('@').to_lowercase();
    matches!(n.as_str(), "ken" | "human" | "user" | "")
        || HUMAN_OWNER.get().is_some_and(|h| *h == n)
}

/// Task lifecycle states — maps 1:1 to Obsidian 1.14 Bases Kanban columns.
/// Includes `PendingClose` so AI agents submit work for evaluation/sign-off before `Done`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Inbox,
    Ready,
    InProgress,
    PendingClose,
    Blocked,
    Done,
    Archived,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Ready => "ready",
            Self::InProgress => "in-progress",
            Self::PendingClose => "pending-close",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }

    #[allow(dead_code)]
    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "inbox" => Self::Inbox,
            "ready" | "todo" | "open" => Self::Ready,
            "in-progress" | "in_progress" | "doing" | "active" => Self::InProgress,
            "pending-close" | "pending_close" | "review" | "verify" => Self::PendingClose,
            "blocked" | "waiting" => Self::Blocked,
            "done" | "completed" | "closed" => Self::Done,
            "archived" => Self::Archived,
            _ => Self::Ready,
        }
    }
}

/// YAML frontmatter stored at the top of each `tasks/tk-XXX-*.md` card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFrontmatter {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: TaskStatus,
    pub priority: String,
    pub owner: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topic: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// Estimated or recorded task duration (e.g. "45s", "15m", "2h", "3d")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Active/latest agent session ID or full resume command (e.g. "8y9380573586" or "agy -c")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Task IDs that this task depends on (must be done before this can start)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// Downstream task IDs waiting on this task
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    /// History of all agent sessions that worked on this task across handoffs (e.g. ["agy: a9b656ba...", "hermes: 8y9380573586"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
    /// Definition of Done criteria (human/agent acceptance checklist)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dod: Vec<String>,
    /// Executable shell command to verify task completion (exit 0 = pass)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_status: Option<String>,
    /// The `eval` command as it stood when the task was claimed, so a reviewer can see
    /// whether the agent changed its own check while working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_eval: Option<String>,
    /// What the agent submitted for review: the exact output snapshot and the check that passed on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ReviewReceipt>,
    /// The eval command a human approved for auto-acceptance (`tk auto`). When the task's
    /// current `eval` equals this and passes on submit, the task closes without a manual sign-off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_eval: Option<String>,
    /// How the final sign-off was given (`interactive` or `auto:eval`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    pub created: String,
    pub updated: String,
    #[serde(default = "default_true")]
    pub _organized: bool,
}

/// Recorded at `pending-close`: binds the eval result to the exact output under review.
/// `snapshot` is a git tree hash of the working tree (task cards excluded); `None` outside a git repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval: Option<String>,
    /// `passed` or `no-eval`
    pub eval_result: String,
    pub submitted_by: String,
    pub submitted_at: String,
}

/// Verified CLI command to CONTINUE the most recent session for a given agent in the current workspace.
pub fn agent_continue_cmd(agent: &str) -> Option<String> {
    let a = agent.trim().trim_start_matches('@').to_lowercase();
    if is_human(&a) {
        return None;
    }
    match a.as_str() {
        "agy" | "antigravity" => Some("agy -c".to_string()),
        "claude" | "claude-code" => Some("claude -c".to_string()),
        "codex" => Some("codex resume --last".to_string()),
        "hermes" => Some("hermes -c".to_string()),
        "opencode" | "oc" => Some("opencode -c".to_string()),
        "gemini" => Some("gemini -r latest".to_string()),
        "pi" => Some("pi -c".to_string()),
        other => Some(format!("{} -c", other)),
    }
}

/// Verified CLI command to RESUME a specific session ID (or continue if `sid` is "latest"/"-c"/empty) for a given agent.
pub fn agent_resume_cmd(agent: &str, sid: Option<&str>) -> Option<String> {
    let a = agent.trim().trim_start_matches('@').to_lowercase();
    let raw = sid.map(str::trim).unwrap_or("");
    if raw.is_empty()
        || matches!(
            raw.to_lowercase().as_str(),
            "latest" | "last" | "continue" | "-c" | "--continue"
        )
    {
        return agent_continue_cmd(&a);
    }
    // If user stored a full command string (e.g. "agy -c" or "claude -r 8y9380573586"), return as-is
    if raw.contains(' ') {
        return Some(raw.to_string());
    }
    if is_human(&a) {
        return None;
    }
    let cmd = match a.as_str() {
        "agy" | "antigravity" => format!("agy --conversation {}", raw),
        "claude" | "claude-code" => format!("claude -r {}", raw),
        "codex" => format!("codex resume {}", raw),
        "hermes" => format!("hermes -r {}", raw),
        "opencode" | "oc" => format!("opencode -s {}", raw),
        "gemini" => format!("gemini -r {}", raw),
        "pi" => format!("pi --session {}", raw),
        other => format!("{} -r {}", other, raw),
    };
    Some(cmd)
}

impl TaskFrontmatter {
    /// Synthesize the runnable CLI resume command for this task's active agent session.
    /// Falls back to the agent's `--continue` command (e.g. `agy -c`) if the owner is an AI agent without an explicit ID.
    pub fn resume_command(&self) -> Option<String> {
        if let Some(ref sid) = self.session_id
            && !sid.trim().is_empty()
        {
            if sid.trim().contains(' ') {
                return Some(sid.trim().to_string());
            }
            // Even if current owner is "ken" (e.g. during sign-off), check `sessions` history for which agent owns this `sid`
            for entry in self.sessions.iter().rev() {
                if let Some((ag, rest)) = entry.split_once(':')
                    && rest.contains(sid.trim())
                {
                    return agent_resume_cmd(ag.trim(), Some(sid.trim()));
                }
            }
            if let Some(cmd) = agent_resume_cmd(&self.owner, Some(sid.trim())) {
                return Some(cmd);
            }
        }
        agent_continue_cmd(&self.owner)
    }

    /// Synthesize the `--continue` command for this task's agent (e.g. `agy -c`, `claude -c`, `codex resume --last`).
    pub fn continue_command(&self) -> Option<String> {
        if let Some(cmd) = agent_continue_cmd(&self.owner) {
            return Some(cmd);
        }
        // If current owner is human ("ken"), check the most recent agent in `sessions`
        for entry in self.sessions.iter().rev() {
            if let Some((ag, _)) = entry.split_once(':')
                && let Some(cmd) = agent_continue_cmd(ag.trim())
            {
                return Some(cmd);
            }
        }
        None
    }
}

fn default_true() -> bool {
    true
}

/// Scope of the task card: repository-local project task or global workspace/vault task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum CardScope {
    Local,
    #[default]
    Global,
}


impl std::fmt::Display for CardScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Global => write!(f, "global"),
        }
    }
}

/// Complete in-memory representation of a task card (YAML frontmatter + Markdown body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCard {
    #[serde(flatten)]
    pub meta: TaskFrontmatter,
    pub body: String,
    pub path: String,
    #[serde(default)]
    pub scope: CardScope,
}

/// Append-only audit event written to `tasks/.events.jsonl` for DuckDB analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub timestamp: String,
    pub id: String,
    pub action: String,
    pub actor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub from_status: Option<String>,
    pub to_status: String,
    pub eval_passed: Option<bool>,
    pub title: String,
}
