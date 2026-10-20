use crate::model::{
    CardScope, ReviewReceipt, TaskCard, TaskEvent, TaskFrontmatter, TaskStatus, agent_continue_cmd,
    agent_resume_cmd, is_human, set_human_owner,
};
use crate::todotxt::parse_todotxt;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

/// Advisory exclusive lock on a card. The lock lives as long as this value: the `File`
/// must stay open, because closing the descriptor releases the `flock`.
#[cfg(unix)]
pub struct CardLock {
    file: std::fs::File,
}

#[cfg(unix)]
impl CardLock {
    pub fn acquire_exclusive(path: &Path) -> Result<Self> {
        use std::os::unix::io::AsRawFd;
        const LOCK_EX: i32 = 2;

        // Hidden sibling (`.tk-001-x.lock`) so lock files stay out of Obsidian and `ls`.
        let lock_path = if path.is_file() || path.extension().is_some() {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("card");
            path.with_file_name(format!(".{}.lock", stem))
        } else {
            path.join(".lock")
        };

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open lock file {:?}", lock_path))?;

        let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX) };
        if rc != 0 {
            bail!("Failed to acquire exclusive lock on {:?}", lock_path);
        }
        Ok(Self { file })
    }
}

#[cfg(unix)]
impl Drop for CardLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        const LOCK_UN: i32 = 8;
        unsafe { flock(self.file.as_raw_fd(), LOCK_UN) };
    }
}

/// No-op on non-Unix platforms: concurrent agents are NOT serialized on Windows.
#[cfg(not(unix))]
pub struct CardLock;

#[cfg(not(unix))]
impl CardLock {
    pub fn acquire_exclusive(_path: &Path) -> Result<Self> {
        Ok(Self)
    }
}

fn default_mode() -> String {
    "obsidian".to_string()
}

fn default_base_file() -> String {
    "bases/tasks.base".to_string()
}

/// Auto-detect an Obsidian vault by checking for a `.obsidian` directory in:
/// 1. Current working directory (or any parent directory)
/// 2. `~/Documents`
/// 3. `~/Obsidian` or `~/Notes`
pub fn detect_obsidian_vault() -> Option<PathBuf> {
    if let Ok(mut cwd) = std::env::current_dir() {
        loop {
            if cwd.join(".obsidian").is_dir() {
                return Some(cwd);
            }
            if !cwd.pop() {
                break;
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        for candidate in ["Documents", "Obsidian", "Notes", "vault"] {
            let p = home_path.join(candidate);
            if p.join(".obsidian").is_dir() {
                return Some(p);
            }
        }
    }
    None
}

/// Auto-detect a local project store by checking for `.tkmd.toml`, `tkmd.toml`, `.tkmd/config.toml`,
/// or legacy `.taskmd.toml`, `taskmd.toml`, `.taskmd/config.toml`,
/// or a `.git` root containing a `tasks/` directory.
/// Skips if the detected directory is the global Obsidian vault path.
pub fn detect_local_store(cwd: &Path, global_vault_path: &Path) -> Option<PathBuf> {
    let mut cur = cwd.to_path_buf();
    let global_canon = global_vault_path.canonicalize().ok();

    loop {
        if let Some(ref g) = global_canon {
            if let Ok(c) = cur.canonicalize()
                && c == *g {
                    return None;
                }
        } else if cur == global_vault_path {
            return None;
        }

        if cur.join(".tkmd.toml").is_file()
            || cur.join("tkmd.toml").is_file()
            || cur.join(".tkmd/config.toml").is_file()
            || cur.join(".taskmd.toml").is_file()
            || cur.join("taskmd.toml").is_file()
            || cur.join(".taskmd/config.toml").is_file()
        {
            return Some(cur);
        }

        if cur.join(".git").exists() && cur.join("tasks").is_dir() {
            return Some(cur);
        }

        if !cur.pop() {
            break;
        }
    }
    None
}

/// Auto-detect the root directory of a git repository by checking for `.git` upwards.
pub fn detect_git_root(cwd: &Path) -> Option<PathBuf> {
    let mut cur = cwd.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

/// Scope selection for command execution:
/// - `Auto`: default behavior (detects local project store if in a repo, otherwise global vault)
/// - `Local` (`-L`, `--local`): forces local repository project store
/// - `Global` (`-G`, `--global`): forces central global vault store
/// - `All` (`-A`, `--all-scopes`): aggregates cards across both local and global stores
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeSelection {
    Auto,
    Local,
    Global,
    All,
}

impl ScopeSelection {
    pub fn from_flags(local: bool, global: bool, all: bool) -> Self {
        if all {
            Self::All
        } else if local {
            Self::Local
        } else if global {
            Self::Global
        } else {
            Self::Auto
        }
    }
}

/// Check whether a CLI binary exists on `$PATH`.
fn command_exists(bin: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(bin).is_file() {
                return true;
            }
        }
    }
    false
}

/// Auto-detect all installed editors on `$PATH` (Helix, Neovim, Zed, VS Code, Cursor, Vim, Micro, Nano).
pub fn detect_installed_editors() -> Vec<String> {
    let candidates = ["hx", "nvim", "zed", "code", "cursor", "vim", "micro", "nano"];
    candidates
        .iter()
        .filter(|b| command_exists(b))
        .map(|s| s.to_string())
        .collect()
}

/// Auto-detect the user's preferred editor from `$VISUAL`, `$EDITOR`, or installed binaries.
pub fn detect_editor() -> String {
    for env_key in ["VISUAL", "EDITOR"] {
        if let Ok(val) = std::env::var(env_key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    detect_installed_editors()
        .into_iter()
        .next()
        .unwrap_or_else(|| "vim".to_string())
}

/// Configuration loaded from `~/.config/tkmd/config.toml` (or legacy `~/.config/taskmd/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// "obsidian" (Recommended: Kanban Base + Daily Note + CLI + MCP) or "cli" (Standalone CLI + MCP)
    #[serde(default = "default_mode")]
    pub mode: String,
    pub vault_path: String,
    pub tasks_dir: String,
    #[serde(default = "default_base_file")]
    pub base_file: String,
    #[serde(default)]
    pub daily_dir: String,
    pub default_owner: String,
    #[serde(default)]
    pub obsidian_vault_name: Option<String>,
    /// Preferred editor for `tk e` and CLI-mode `tk o` (e.g., "hx", "nvim", "zed", "code", "vim")
    #[serde(default)]
    pub editor: Option<String>,
    /// Lifecycle guardrails (`[policy]` table). Must stay the last field: TOML tables follow values.
    #[serde(default)]
    pub policy: Policy,
}

/// `[policy]` guardrails for the submit -> review -> accept lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Refuse `pending-close` for tasks with no `eval` command (instead of auto-passing as `no-eval`).
    #[serde(default)]
    pub require_eval: bool,
    /// Kill an eval (and everything it started) after this many seconds; `0` means no limit.
    /// Evals run under the card lock, so a hung eval would otherwise block that card forever.
    #[serde(default = "default_eval_timeout")]
    pub eval_timeout_secs: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            require_eval: false,
            eval_timeout_secs: default_eval_timeout(),
        }
    }
}

fn default_eval_timeout() -> u64 {
    600
}

/// How a caller asks to close a task. Only the CLI, after checking for a human at a
/// terminal, constructs `Interactive`; MCP and scripts always pass `Untrusted`.
/// This is a workflow harness, not authentication: an agent that fakes a terminal passes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Agents, MCP and any non-interactive caller: can submit for review, can never mark done.
    Untrusted,
    /// A human confirmed at an interactive terminal.
    Interactive,
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let user = std::env::var("USER").unwrap_or_else(|_| "owner".to_string());
        let editor = Some(detect_editor());

        if let Some(vault) = detect_obsidian_vault() {
            let vault_name = vault
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string());
            let base_file = if vault.join("bases").is_dir() || vault.join("bases/tasks.base").exists() {
                "bases/tasks.base".to_string()
            } else {
                "tasks.base".to_string()
            };
            Self {
                mode: "obsidian".to_string(),
                vault_path: vault.to_string_lossy().to_string(),
                tasks_dir: "tasks".to_string(),
                base_file,
                daily_dir: "journals/daily".to_string(),
                default_owner: user,
                obsidian_vault_name: vault_name,
                editor,
                policy: Policy::default(),
            }
        } else {
            Self {
                mode: "cli".to_string(),
                vault_path: format!("{}/.local/share/tkmd", home),
                tasks_dir: "tasks".to_string(),
                base_file: String::new(),
                daily_dir: String::new(),
                default_owner: user,
                obsidian_vault_name: None,
                editor,
                policy: Policy::default(),
            }
        }
    }
}

pub struct TaskStore {
    pub config: AppConfig,
    pub tasks_path: PathBuf,
    pub daily_path: PathBuf,
    pub scope: CardScope,
    pub peer: Option<Box<TaskStore>>,
    pub active_scope: ScopeSelection,
    pub uninitialized_git_repo: Option<PathBuf>,
    pub is_uninitialized_local: bool,
}

impl TaskStore {
    pub fn config_file_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let modern = PathBuf::from(&home).join(".config/tkmd/config.toml");
        if modern.is_file() {
            return modern;
        }
        let legacy = PathBuf::from(&home).join(".config/taskmd/config.toml");
        if legacy.is_file() {
            return legacy;
        }
        modern
    }

    pub fn load_global_store() -> Result<Self> {
        let cfg_path = Self::config_file_path();

        let config: AppConfig = if cfg_path.exists() {
            let raw = fs::read_to_string(&cfg_path)
                .with_context(|| format!("Failed reading {:?}", cfg_path))?;
            toml::from_str(&raw).with_context(|| format!("Invalid tkmd config {:?}", cfg_path))?
        } else {
            let def = AppConfig::default();
            if let Some(parent) = cfg_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&cfg_path, toml::to_string_pretty(&def).unwrap_or_default());
            def
        };

        let mut config = config;
        set_human_owner(&config.default_owner);
        // Check for runtime environment variable override (TASKMD_DIR or TASKMD_PATH)
        if let Ok(env_path) = std::env::var("TASKMD_DIR").or_else(|_| std::env::var("TASKMD_PATH")) {
            config.vault_path = env_path;
            config.tasks_dir = String::new();
        }

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let expanded_vault = if config.vault_path.starts_with("~/") {
            format!("{}/{}", home, &config.vault_path[2..])
        } else {
            config.vault_path.clone()
        };

        let vault = PathBuf::from(&expanded_vault);
        let tasks_path = if config.tasks_dir.is_empty() {
            vault.clone()
        } else {
            vault.join(&config.tasks_dir)
        };
        let daily_path = if config.daily_dir.is_empty() {
            vault.clone()
        } else {
            vault.join(&config.daily_dir)
        };
        fs::create_dir_all(&tasks_path)?;

        Ok(Self {
            config,
            tasks_path,
            daily_path,
            scope: CardScope::Global,
            peer: None,
            active_scope: ScopeSelection::Global,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        })
    }

    pub fn load_local_store(root: &Path) -> Result<Self> {
        let user = std::env::var("USER").unwrap_or_else(|_| "owner".to_string());
        let cfg_file = if root.join(".tkmd.toml").is_file() {
            Some(root.join(".tkmd.toml"))
        } else if root.join("tkmd.toml").is_file() {
            Some(root.join("tkmd.toml"))
        } else if root.join(".tkmd/config.toml").is_file() {
            Some(root.join(".tkmd/config.toml"))
        } else if root.join(".taskmd.toml").is_file() {
            Some(root.join(".taskmd.toml"))
        } else if root.join("taskmd.toml").is_file() {
            Some(root.join("taskmd.toml"))
        } else if root.join(".taskmd/config.toml").is_file() {
            Some(root.join(".taskmd/config.toml"))
        } else {
            None
        };

        let config: AppConfig = if let Some(p) = cfg_file {
            let raw = fs::read_to_string(&p)
                .with_context(|| format!("Failed reading local config {:?}", p))?;
            toml::from_str(&raw).with_context(|| format!("Invalid tkmd config {:?}", p))?
        } else {
            AppConfig {
                mode: "cli".to_string(),
                vault_path: root.to_string_lossy().to_string(),
                tasks_dir: "tasks".to_string(),
                base_file: String::new(),
                daily_dir: String::new(),
                default_owner: user,
                obsidian_vault_name: None,
                editor: Some(detect_editor()),
                policy: Policy::default(),
            }
        };

        set_human_owner(&config.default_owner);
        let root_path = PathBuf::from(&config.vault_path);
        let tasks_path = if config.tasks_dir.is_empty() {
            root_path.clone()
        } else {
            root_path.join(&config.tasks_dir)
        };
        let daily_path = if config.daily_dir.is_empty() {
            root_path.clone()
        } else {
            root_path.join(&config.daily_dir)
        };
        Ok(Self {
            config,
            tasks_path,
            daily_path,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        })
    }

    pub fn load_with_scope(scope_sel: ScopeSelection) -> Result<Self> {
        let global = Self::load_global_store()?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let global_vault = PathBuf::from(&global.config.vault_path);
        let local_root = detect_local_store(&cwd, &global_vault);

        match scope_sel {
            ScopeSelection::Global => {
                let mut store = global;
                store.active_scope = ScopeSelection::Global;
                Ok(store)
            }
            ScopeSelection::Local => {
                let is_uninit = local_root.is_none();
                let root = if let Some(r) = local_root {
                    r
                } else if let Some(gr) = detect_git_root(&cwd) {
                    gr
                } else {
                    cwd.clone()
                };
                let mut local = Self::load_local_store(&root)?;
                local.peer = Some(Box::new(global));
                local.active_scope = ScopeSelection::Local;
                local.is_uninitialized_local = is_uninit;
                Ok(local)
            }
            ScopeSelection::All => {
                if let Some(r) = local_root.or_else(|| detect_git_root(&cwd)) {
                    let mut local = Self::load_local_store(&r)?;
                    local.peer = Some(Box::new(global));
                    local.active_scope = ScopeSelection::All;
                    Ok(local)
                } else {
                    let mut store = global;
                    store.active_scope = ScopeSelection::All;
                    Ok(store)
                }
            }
            ScopeSelection::Auto => {
                if let Some(r) = local_root {
                    let mut local = Self::load_local_store(&r)?;
                    local.peer = Some(Box::new(global));
                    local.active_scope = ScopeSelection::Auto;
                    Ok(local)
                } else {
                    let mut store = global;
                    store.active_scope = ScopeSelection::Auto;
                    store.uninitialized_git_repo = detect_git_root(&cwd);
                    Ok(store)
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn load() -> Result<Self> {
        Self::load_with_scope(ScopeSelection::Auto)
    }

    pub fn open_in_editor(&self, target: &Path) -> Result<()> {
        let ed = self
            .config
            .editor
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(detect_editor);
        let parts: Vec<&str> = ed.split_whitespace().collect();
        if let Some((bin, args)) = parts.split_first() {
            Command::new(bin).args(args).arg(target).status()?;
        }
        Ok(())
    }

    /// Initialize either an Obsidian Vault setup (recommended) or a Standalone CLI setup.
    pub fn init_workspace(
        mode: &str,
        target_path: &str,
        owner: &str,
        base_file: &str,
        daily_dir: &str,
        editor: Option<String>,
        is_local: bool,
    ) -> Result<(AppConfig, PathBuf)> {
        let expanded = if let Some(stripped) = target_path.strip_prefix("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{}/{}", home, stripped)
        } else {
            target_path.to_string()
        };
        let root = PathBuf::from(&expanded);
        let tasks_dir = "tasks".to_string();
        fs::create_dir_all(root.join(&tasks_dir))?;

        let vault_name = if mode == "obsidian" {
            root.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        } else {
            None
        };

        if mode == "obsidian" {
            // 1. Ensure bases/tasks.base exists
            let base_path = root.join(base_file);
            if let Some(parent) = base_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if !base_path.exists() {
                let base_yaml = format!(
                    r#"filters:
  and:
    - file.inFolder("tasks")
    - type == "task"
views:
  - type: cards
    name: Kanban Board
    groupBy:
      property: status
      direction: ASC
    order:
      - file.name
      - priority
      - owner
      - session_id
      - sessions
      - eval_status
      - project
      - due
    sort:
      - property: priority
        direction: ASC
  - type: table
    name: Pending Close (Sign-Off Queue)
    filters:
      and:
        - status == "pending-close"
    order:
      - id
      - file.name
      - owner
      - session_id
      - sessions
      - eval_status
      - dod
      - updated
    sort:
      - property: priority
        direction: ASC
  - type: table
    name: {owner}'s Queue
    filters:
      and:
        - owner == "{owner}"
        - status != "done"
    order:
      - id
      - file.name
      - priority
      - status
      - dod
      - due
    sort:
      - property: priority
        direction: ASC
  - type: table
    name: Agent Queues
    filters:
      and:
        - owner != "{owner}"
        - status != "done"
    order:
      - id
      - file.name
      - owner
      - session_id
      - sessions
      - priority
      - status
      - eval_status
      - eval
    sort:
      - property: priority
        direction: ASC
  - type: table
    name: Completed Tasks
    filters:
      and:
        - status == "done"
    order:
      - id
      - file.name
      - owner
      - eval_status
      - updated
    sort:
      - property: updated
        direction: DESC
"#,
                    owner = owner
                );
                fs::write(&base_path, base_yaml)?;
            }

            // 2. Safely merge property types into .obsidian/types.json if .obsidian exists
            let obs_dir = root.join(".obsidian");
            if obs_dir.exists() {
                let types_path = obs_dir.join("types.json");
                let mut root_json: serde_json::Value = if types_path.exists() {
                    let raw = fs::read_to_string(&types_path).unwrap_or_else(|_| "{}".to_string());
                    serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({"types": {}}))
                } else {
                    serde_json::json!({"types": {}})
                };
                if root_json.get("types").is_none() {
                    root_json["types"] = serde_json::json!({});
                }
                if let Some(types_obj) = root_json["types"].as_object_mut() {
                    let defaults = [
                        ("id", "text"),
                        ("status", "text"),
                        ("priority", "text"),
                        ("owner", "text"),
                        ("session_id", "text"),
                        ("project", "text"),
                        ("due", "date"),
                        ("dod", "multitext"),
                        ("eval", "text"),
                        ("eval_status", "text"),
                    ];
                    for (k, v) in defaults {
                        types_obj
                            .entry(k.to_string())
                            .or_insert_with(|| serde_json::Value::String(v.to_string()));
                    }
                }
                let _ = fs::write(&types_path, serde_json::to_string_pretty(&root_json)?);
            }
        }

        let cfg = AppConfig {
            mode: mode.to_string(),
            vault_path: expanded,
            tasks_dir,
            base_file: if mode == "obsidian" {
                base_file.to_string()
            } else {
                String::new()
            },
            daily_dir: if mode == "obsidian" {
                daily_dir.to_string()
            } else {
                String::new()
            },
            default_owner: owner.to_string(),
            obsidian_vault_name: vault_name,
            editor: editor.or_else(|| Some(detect_editor())),
            policy: Policy::default(),
        };

        let cfg_path = if is_local {
            root.join(".tkmd.toml")
        } else {
            Self::config_file_path()
        };
        if let Some(parent) = cfg_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&cfg_path, toml::to_string_pretty(&cfg)?)?;

        Ok((cfg, cfg_path))
    }

    pub fn list_store_cards(&self) -> Result<Vec<TaskCard>> {
        let mut cards = Vec::new();
        if !self.tasks_path.exists() {
            return Ok(cards);
        }
        for entry in fs::read_dir(&self.tasks_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if fname.starts_with('.') || fname.eq_ignore_ascii_case("readme.md") {
                continue;
            }
            match Self::read_card(&path) {
                Ok(mut card) => {
                    card.scope = self.scope;
                    cards.push(card);
                }
                Err(e) => warn_unreadable_card(&path, &e),
            }
        }
        cards.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        Ok(cards)
    }

    pub fn list_all(&self) -> Result<Vec<TaskCard>> {
        let mut cards = self.list_store_cards()?;
        if self.active_scope == ScopeSelection::All
            && let Some(ref peer) = self.peer {
                let mut peer_cards = peer.list_store_cards()?;
                cards.append(&mut peer_cards);
                cards.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
            }
        Ok(cards)
    }

    pub fn read_card(path: &Path) -> Result<TaskCard> {
        let raw = fs::read_to_string(path)?;
        if !raw.starts_with("---\n") && !raw.starts_with("---\r\n") {
            bail!("Missing YAML frontmatter in {:?}", path);
        }
        let end_idx = raw[4..]
            .find("\n---")
            .context("Unclosed YAML frontmatter")?
            + 4;
        let yaml_str = &raw[4..end_idx];
        let body = raw[end_idx + 4..].trim_start_matches(['\r', '\n']).to_string();

        let meta: TaskFrontmatter = serde_yaml::from_str(yaml_str)
            .with_context(|| format!("Invalid YAML in {:?}", path))?;

        Ok(TaskCard {
            meta,
            body,
            path: path.to_string_lossy().to_string(),
            scope: CardScope::default(),
        })
    }

    /// Acquire an advisory exclusive lock on a task card to serialize concurrent agent updates.
    pub fn lock_card(path: &Path) -> Result<CardLock> {
        CardLock::acquire_exclusive(path)
    }

    pub fn write_card(&self, card: &TaskCard) -> Result<()> {
        let mut c = card.clone();
        Self::sync_card_sessions(&mut c);
        let yaml = serde_yaml::to_string(&c.meta)?;
        let content = format!("---\n{}---\n\n{}\n", yaml, c.body.trim());

        // POSIX atomic write: write to sibling temp file, then atomic rename
        let target_path = Path::new(&c.path);
        let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
        let file_stem = target_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("card");
        let tmp_path = parent.join(format!(".{}.tmp.{}", file_stem, std::process::id()));

        fs::write(&tmp_path, content)?;
        fs::rename(&tmp_path, target_path)?;
        Ok(())
    }

    /// Record or update an agent session entry on the card (`meta.session_id` + `meta.sessions`).
    pub fn record_session_on_card(card: &mut TaskCard, actor: &str, sid: Option<&str>) {
        let clean_actor = actor.trim().trim_start_matches('@').to_lowercase();
        let clean_sid = sid.map(str::trim).filter(|s| !s.is_empty());

        if let Some(s) = clean_sid {
            card.meta.session_id = Some(s.to_string());
        }

        // Determine which agent CLI owns this session
        let agent = if !is_human(&clean_actor) && clean_actor != "agent" {
            clean_actor
        } else if let Some(s) = clean_sid
            && let Some(first_word) = s.split_whitespace().next()
            && matches!(
                first_word.to_lowercase().as_str(),
                "agy" | "claude" | "codex" | "hermes" | "opencode" | "gemini" | "pi"
            )
        {
            first_word.to_lowercase()
        } else if !is_human(&card.meta.owner) && !card.meta.owner.eq_ignore_ascii_case("agent") {
            card.meta.owner.to_lowercase()
        } else {
            return;
        };

        let entry_str = if let Some(s) = clean_sid {
            format!("{}: {}", agent, s)
        } else {
            format!("{}: latest", agent)
        };

        // Update existing entry for the same agent or append a new agent session entry
        let agent_prefix = format!("{}:", agent);
        let prefix_latest = format!("{}: latest", agent);
        if let Some(existing) = card.meta.sessions.iter_mut().find(|e| {
            *e == &entry_str
                || (clean_sid.is_some() && (*e == &prefix_latest || e.starts_with(&agent_prefix)))
        }) {
            if clean_sid.is_some() || *existing == prefix_latest {
                *existing = entry_str;
            }
        } else if (!card.meta.sessions.iter().any(|e| e.starts_with(&agent_prefix))
            || clean_sid.is_some())
            && !card.meta.sessions.contains(&entry_str) {
                card.meta.sessions.push(entry_str);
            }
    }

    /// Synchronize `meta.sessions` and the `## Agent Sessions` Markdown section inside `card.body`
    /// so a human viewing the `.md` file in Obsidian or `$EDITOR` always sees copy-pasteable
    /// `resume` and `continue` commands across every agent that touched the task.
    pub fn sync_card_sessions(card: &mut TaskCard) {
        let owner_copy = card.meta.owner.clone();
        let sid_copy = card.meta.session_id.clone();
        let sid_already_assigned = sid_copy.as_deref().is_some_and(|s| {
            card.meta
                .sessions
                .iter()
                .any(|entry| entry.split_once(':').is_some_and(|(_, es)| es.trim() == s.trim()))
        });
        if !sid_already_assigned {
            Self::record_session_on_card(card, &owner_copy, sid_copy.as_deref());
        } else {
            Self::record_session_on_card(card, &owner_copy, None);
        }

        if card.meta.sessions.is_empty() {
            return;
        }

        let mut md_lines = Vec::new();
        md_lines.push("## Agent Sessions".to_string());
        for entry in &card.meta.sessions {
            let (agent, raw_sid) = entry
                .split_once(':')
                .map(|(a, s)| (a.trim(), s.trim()))
                .unwrap_or((card.meta.owner.as_str(), entry.as_str()));

            let resume_opt = agent_resume_cmd(agent, Some(raw_sid));
            let cont_opt = agent_continue_cmd(agent);

            match (resume_opt, cont_opt) {
                (Some(res), Some(cont)) if res != cont => {
                    md_lines.push(format!(
                        "- **@{}** (`{}`) — resume: `{}` · continue: `{}`",
                        agent, raw_sid, res, cont
                    ));
                }
                (Some(res), _) => {
                    md_lines.push(format!(
                        "- **@{}** (`{}`) — continue: `{}`",
                        agent, raw_sid, res
                    ));
                }
                _ => {
                    md_lines.push(format!("- **@{}**: `{}`", agent, raw_sid));
                }
            }
        }
        let section_block = md_lines.join("\n");

        if let Some(start_idx) = card.body.find("## Agent Sessions") {
            let after_header = &card.body[start_idx + "## Agent Sessions".len()..];
            if let Some(next_sec_rel) = after_header.find("\n## ") {
                let end_idx = start_idx + "## Agent Sessions".len() + next_sec_rel;
                card.body = format!(
                    "{}{}\n{}",
                    &card.body[..start_idx],
                    section_block,
                    &card.body[end_idx..]
                );
            } else {
                card.body = format!("{}{}\n", &card.body[..start_idx], section_block);
            }
        } else if let Some(exec_idx) = card.body.find("\n## Execution Log") {
            card.body = format!(
                "{}\n\n{}\n\n{}",
                card.body[..exec_idx].trim_end(),
                section_block,
                &card.body[exec_idx + 1..]
            );
        } else {
            card.body = format!("{}\n\n{}\n", card.body.trim_end(), section_block);
        }
    }

    pub fn next_id(&self) -> Result<String> {
        let cards = self.list_all()?;
        let max_num = cards
            .iter()
            .filter_map(|c| c.meta.id.strip_prefix("tk-"))
            .filter_map(|n| n.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        Ok(format!("tk-{:03}", max_num + 1))
    }

    /// Auto-detect agent session ID from common environment variables if not explicitly passed.
    pub fn detect_env_session_id() -> Option<String> {
        for var in [
            "AGY_CONVERSATION_ID",
            "ANTIGRAVITY_CONVERSATION_ID",
            "CLAUDE_SESSION_ID",
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "HERMES_SESSION_ID",
            "OPENCODE_SESSION_ID",
            "PI_SESSION_ID",
            "GEMINI_SESSION_ID",
            "AGENT_SESSION_ID",
            "SESSION_ID",
        ] {
            if let Ok(val) = std::env::var(var) {
                let t = val.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    }

    pub fn add_task(
        &self,
        raw_input: &str,
        details: Option<&str>,
        dod: Vec<String>,
        eval: Option<String>,
        session_id: Option<String>,
    ) -> Result<TaskCard> {
        let parsed = parse_todotxt(raw_input, &self.config.default_owner);
        if parsed.title.is_empty() {
            bail!("Task title cannot be empty");
        }
        // Serialize id allocation across processes: without this, racing agents reuse ids.
        fs::create_dir_all(&self.tasks_path)?;
        let _store_lock = Self::lock_card(&self.tasks_path)?;
        let id = self.next_id()?;
        let slug = slugify(&parsed.title);
        let filename = format!("{}-{}.md", id, slug);
        let full_path = self.tasks_path.join(&filename);
        let today = Local::now().format("%Y-%m-%d").to_string();

        let topic = parsed
            .tags
            .iter()
            .filter(|t| *t != "task")
            .cloned()
            .collect::<Vec<_>>();

        let eval_status = if eval.is_some() {
            Some("pending".to_string())
        } else {
            None
        };

        let resolved_session = session_id
            .filter(|s| !s.trim().is_empty())
            .or(parsed.session_id)
            .or_else(Self::detect_env_session_id);

        let initial_status = if !parsed.blocked_by.is_empty() {
            TaskStatus::Blocked
        } else {
            TaskStatus::Ready
        };

        let meta = TaskFrontmatter {
            id: id.clone(),
            title: parsed.title.clone(),
            kind: "task".to_string(),
            status: initial_status,
            priority: parsed.priority,
            owner: parsed.owner.clone(),
            topic,
            tags: parsed.tags,
            project: parsed.project,
            due: parsed.due,
            duration: parsed.duration,
            session_id: resolved_session,
            blocked_by: parsed.blocked_by,
            blocks: Vec::new(),
            sessions: Vec::new(),
            dod: dod.clone(),
            eval: eval.clone(),
            eval_status,
            claimed_eval: None,
            receipt: None,
            approval: None,
            approved_eval: None,
            created: today.clone(),
            updated: today,
            _organized: true,
        };

        let mut sections = format!("# {}\n", meta.title);
        if let Some(d) = details
            && !d.trim().is_empty() {
                sections.push_str(&format!("\n{}\n", d.trim()));
            }
        if !dod.is_empty() {
            sections.push_str("\n## Definition of Done (DoD)\n");
            for item in &dod {
                sections.push_str(&format!("- [ ] {}\n", item));
            }
        }
        if let Some(ev) = &eval {
            sections.push_str(&format!("\n## Automated Eval\n```bash\n{}\n```\n", ev));
        }
        let session_note = meta
            .resume_command()
            .map(|cmd| format!(" (resume: `{}`)", cmd))
            .unwrap_or_default();
        sections.push_str(&format!(
            "\n## Execution Log\n- Created and assigned to `{}`{}.",
            meta.owner, session_note
        ));

        let mut card = TaskCard {
            meta,
            body: sections,
            path: full_path.to_string_lossy().to_string(),
            scope: self.scope,
        };
        Self::sync_card_sessions(&mut card);
        self.write_card(&card)?;
        self.log_event(
            &card.meta.id,
            "created",
            &card.meta.owner,
            card.meta.session_id.as_deref(),
            None,
            "ready",
            None,
            &card.meta.title,
        )?;
        Ok(card)
    }

    /// Resolve a task by id (`7`, `tk-7`, `tk-007`) or by a unique fragment of its file name.
    /// Ids match exactly. A name fragment matching several cards is an error listing them,
    /// so `tk d smoke` can never close the wrong task.
    pub fn find_task(&self, query: &str) -> Result<TaskCard> {
        let q = query.trim().to_lowercase();
        let digits = q.strip_prefix("tk-").unwrap_or(&q);
        let id_query = digits.parse::<u32>().ok().map(|n| format!("tk-{:03}", n));

        let cards = self.list_store_cards()?;
        if let Some(id) = &id_query {
            if let Some(card) = cards.into_iter().find(|c| c.meta.id.eq_ignore_ascii_case(id)) {
                return Ok(card);
            }
        } else if !q.is_empty() {
            let mut matches: Vec<TaskCard> = cards
                .into_iter()
                .filter(|c| {
                    Path::new(&c.path)
                        .file_stem()
                        .and_then(|f| f.to_str())
                        .is_some_and(|f| f.to_lowercase().contains(&q))
                })
                .collect();
            match matches.len() {
                0 => {}
                1 => return Ok(matches.remove(0)),
                _ => bail!(
                    "'{}' matches {} tasks: {}. Use an id",
                    query,
                    matches.len(),
                    matches
                        .iter()
                        .map(|c| format!("{} ({})", c.meta.id, c.meta.title))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }

        if (self.active_scope == ScopeSelection::Auto || self.active_scope == ScopeSelection::All)
            && let Some(ref peer) = self.peer
        {
            return peer.find_task(query);
        }

        bail!("Task '{}' not found", query)
    }

    /// Claim a task -> `in-progress`.
    /// Refuses tasks under review or closed, and tasks another owner is working on, unless `force`.
    pub fn claim_task(
        &self,
        query: &str,
        owner: &str,
        session_id: Option<String>,
        force: bool,
    ) -> Result<TaskCard> {
        let found = self.find_task(query)?;
        let _lock = Self::lock_card(Path::new(&found.path))?;
        let mut card = Self::read_card(Path::new(&found.path))?;
        card.scope = found.scope;
        let prev = card.meta.status.as_str().to_string();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let now_ts = Local::now().format("%Y-%m-%d %H:%M").to_string();
        let new_owner = owner.trim().trim_start_matches('@').to_lowercase();

        match card.meta.status {
            TaskStatus::Done | TaskStatus::Archived => {
                bail!("Cannot claim {}: it is already `{}`", card.meta.id, prev);
            }
            TaskStatus::PendingClose if !force => {
                bail!(
                    "Cannot claim {}: it is awaiting sign-off (`pending-close`). Use --force to take it back into work",
                    card.meta.id
                );
            }
            TaskStatus::InProgress
                if !force && !card.meta.owner.eq_ignore_ascii_case(&new_owner) =>
            {
                bail!(
                    "Cannot claim {}: `{}` is working on it. Use --force to take it over",
                    card.meta.id,
                    card.meta.owner
                );
            }
            _ => {}
        }

        let open = self.open_blockers(&card)?;
        if !open.is_empty() {
            bail!("Cannot claim {} — Blocked by open task(s): {}", card.meta.id, open.join(", "));
        }

        let takeover = if force && !card.meta.owner.eq_ignore_ascii_case(&new_owner) {
            format!(" (forced takeover from `{}`, was `{}`)", card.meta.owner, prev)
        } else {
            String::new()
        };
        card.meta.status = TaskStatus::InProgress;
        card.meta.owner = new_owner;
        // A new claim starts a new attempt: any earlier review receipt no longer applies.
        card.meta.receipt = None;
        card.meta.claimed_eval = card.meta.eval.clone();
        let resolved_sid = session_id
            .filter(|s| !s.trim().is_empty())
            .or_else(Self::detect_env_session_id);
        Self::record_session_on_card(&mut card, owner, resolved_sid.as_deref());
        card.meta.updated = today;
        let session_note = card
            .meta
            .resume_command()
            .map(|cmd| format!(" — resume: `{}`", cmd))
            .unwrap_or_default();
        card.body = format!(
            "{}\n- `{}`: Claimed by `{}` (`in-progress`){}{}.",
            card.body.trim_end(),
            now_ts,
            owner,
            takeover,
            session_note
        );

        Self::sync_card_sessions(&mut card);
        self.write_card(&card)?;
        self.log_event(
            &card.meta.id,
            if takeover.is_empty() { "claimed" } else { "claimed-forced" },
            owner,
            card.meta.session_id.as_deref(),
            Some(&prev),
            "in-progress",
            None,
            &card.meta.title,
        )?;
        Ok(card)
    }

    /// Ids in `card.blocked_by` that are not yet done or archived (unknown ids count as closed).
    fn open_blockers(&self, card: &TaskCard) -> Result<Vec<String>> {
        if card.meta.blocked_by.is_empty() {
            return Ok(Vec::new());
        }
        let all = self.list_all()?;
        Ok(card
            .meta
            .blocked_by
            .iter()
            .filter(|bid| {
                all.iter()
                    .find(|c| c.meta.id.eq_ignore_ascii_case(bid))
                    .is_some_and(|t| !matches!(t.meta.status, TaskStatus::Done | TaskStatus::Archived))
            })
            .cloned()
            .collect())
    }

    /// Runs the task's `eval` shell command (if configured) and records pass/fail on the card.
    pub fn run_eval(&self, query: &str, actor: &str) -> Result<(TaskCard, bool, String)> {
        let found = self.find_task(query)?;
        let _lock = Self::lock_card(Path::new(&found.path))?;
        let mut card = Self::read_card(Path::new(&found.path))?;
        card.scope = found.scope;
        let (passed, output) = self.eval_card(&mut card, actor)?;
        self.write_card(&card)?;
        let cur_status = card.meta.status.as_str().to_string();
        self.log_event(
            &card.meta.id,
            "eval",
            actor,
            card.meta.session_id.as_deref(),
            Some(&cur_status),
            &cur_status,
            Some(passed),
            &card.meta.title,
        )?;
        Ok((card, passed, output))
    }

    /// Evaluate a card the caller has already locked and re-read. Mutates the card; does not write it.
    fn eval_card(&self, card: &mut TaskCard, actor: &str) -> Result<(bool, String)> {
        let now_ts = Local::now().format("%Y-%m-%d %H:%M").to_string();

        let Some(cmd_str) = card.meta.eval.clone() else {
            card.meta.eval_status = Some("passed (no-eval)".to_string());
            return Ok((true, "No eval command configured (auto-pass)".to_string()));
        };

        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&cmd_str);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let shell = if let Ok(s) = std::env::var("SHELL")
                && !s.is_empty()
                && std::path::Path::new(&s).exists()
            {
                s
            } else if std::path::Path::new("/bin/bash").exists() {
                "/bin/bash".to_string()
            } else if std::path::Path::new("/bin/zsh").exists() {
                "/bin/zsh".to_string()
            } else {
                "sh".to_string()
            };
            let mut c = Command::new(shell);
            c.arg("-c").arg(&cmd_str);
            c
        };

        cmd.current_dir(&self.config.vault_path);
        let timeout = match self.config.policy.eval_timeout_secs {
            0 => None,
            secs => Some(std::time::Duration::from_secs(secs)),
        };
        let output = run_with_timeout(cmd, timeout)
            .with_context(|| format!("Failed to execute eval command: {}", cmd_str))?;
        let Some(output) = output else {
            let secs = self.config.policy.eval_timeout_secs;
            card.meta.eval_status = Some("failed".to_string());
            card.body = format!(
                "{}\n- `{}`: Eval `{}` run by `{}` -> **TIMED OUT ⏱** (after {}s).",
                card.body.trim_end(),
                now_ts,
                cmd_str,
                actor,
                secs
            );
            return Ok((false, format!("eval timed out after {}s ([policy].eval_timeout_secs)", secs)));
        };

        let passed = output.status.success();
        let stdout_s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr_s = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let combined = if !stdout_s.is_empty() && !stderr_s.is_empty() {
            format!("{} | {}", stdout_s, stderr_s)
        } else if !stdout_s.is_empty() {
            stdout_s
        } else if !stderr_s.is_empty() {
            stderr_s
        } else {
            format!("exit {}", output.status.code().unwrap_or(-1))
        };

        card.meta.eval_status = Some(if passed {
            "passed".to_string()
        } else {
            "failed".to_string()
        });
        card.meta.updated = Local::now().format("%Y-%m-%d").to_string();
        let badge = if passed { "PASS ✅" } else { "FAIL ❌" };
        card.body = format!(
            "{}\n- `{}`: Eval `{}` run by `{}` -> **{}** (`{}`).",
            card.body.trim_end(),
            now_ts,
            cmd_str,
            actor,
            badge,
            combined.lines().next().unwrap_or("")
        );
        Ok((passed, combined))
    }

    /// Submits a task to `pending-close`. Runs `eval` under the card lock; a failing eval blocks
    /// the transition. Records a `ReviewReceipt` binding the pass to the exact output snapshot.
    pub fn submit_for_review(
        &self,
        query: &str,
        actor: &str,
        note: Option<&str>,
        session_id: Option<String>,
    ) -> Result<TaskCard> {
        let found = self.find_task(query)?;
        let _lock = Self::lock_card(Path::new(&found.path))?;
        let mut card = Self::read_card(Path::new(&found.path))?;
        card.scope = found.scope;

        if matches!(card.meta.status, TaskStatus::Done | TaskStatus::Archived) {
            bail!("Cannot submit {}: it is already `{}`", card.meta.id, card.meta.status.as_str());
        }
        let open = self.open_blockers(&card)?;
        if !open.is_empty() {
            bail!("Cannot submit {}: still blocked by open task(s): {}", card.meta.id, open.join(", "));
        }
        let submitter = actor.trim().trim_start_matches('@').to_lowercase();
        if card.meta.status == TaskStatus::InProgress && !card.meta.owner.eq_ignore_ascii_case(&submitter) {
            bail!(
                "Cannot submit {}: `{}` claimed it. Only the owner submits; take it over with `tk c {} --force`",
                card.meta.id,
                card.meta.owner,
                card.meta.id
            );
        }
        if self.config.policy.require_eval && card.meta.eval.is_none() {
            bail!(
                "Cannot submit {}: [policy].require_eval is on and the task has no eval command. Add one with `tk set {} --eval '<cmd>'`",
                card.meta.id,
                card.meta.id
            );
        }

        let prev = card.meta.status.as_str().to_string();
        let (passed, eval_out) = self.eval_card(&mut card, actor)?;
        if !passed {
            self.write_card(&card)?;
            self.log_event(
                &card.meta.id,
                "eval",
                actor,
                card.meta.session_id.as_deref(),
                Some(&prev),
                &prev,
                Some(false),
                &card.meta.title,
            )?;
            bail!(
                "Eval failed for {}! Fix before moving to pending-close. Output: {}",
                card.meta.id,
                eval_out
            );
        }

        let now_ts = Local::now().format("%Y-%m-%d %H:%M").to_string();
        let resolved_sid = session_id
            .filter(|s| !s.trim().is_empty())
            .or_else(Self::detect_env_session_id);
        Self::record_session_on_card(&mut card, actor, resolved_sid.as_deref());
        card.meta.receipt = Some(ReviewReceipt {
            snapshot: self.worktree_snapshot(),
            eval: card.meta.eval.clone(),
            eval_result: if card.meta.eval.is_some() { "passed" } else { "no-eval" }.to_string(),
            submitted_by: actor.to_string(),
            submitted_at: Local::now().to_rfc3339(),
        });
        let auto_accept = card.meta.eval.is_some() && card.meta.approved_eval == card.meta.eval;
        card.meta.status = TaskStatus::PendingClose;
        card.meta.updated = Local::now().format("%Y-%m-%d").to_string();
        let suffix = match note {
            Some(n) if !n.trim().is_empty() => format!(" — {}", n.trim()),
            _ => String::new(),
        };
        let session_note = card
            .meta
            .resume_command()
            .map(|cmd| format!(" (resume: `{}`)", cmd))
            .unwrap_or_default();
        card.body = format!(
            "{}\n- `{}`: Submitted for `pending-close` review by `{}` 🔍{}{}.",
            card.body.trim_end(),
            now_ts,
            actor,
            session_note,
            suffix
        );

        Self::sync_card_sessions(&mut card);

        if auto_accept {
            // The human approved this exact check in advance; its pass is the sign-off.
            let today = Local::now().format("%Y-%m-%d").to_string();
            card.meta.status = TaskStatus::Done;
            card.meta.approval = Some("auto:eval".to_string());
            card.body = card.body.replace("- [ ] ", "- [x] ");
            card.body = format!(
                "{}\n- `{}`: Auto-accepted (`done`): eval `{}` approved by `{}` passed ✅.",
                card.body.trim_end(),
                now_ts,
                card.meta.eval.as_deref().unwrap_or(""),
                self.config.default_owner
            );
            self.write_card(&card)?;
            self.log_event(
                &card.meta.id,
                "auto-accepted",
                actor,
                card.meta.session_id.as_deref(),
                Some(&prev),
                "done",
                Some(true),
                &card.meta.title,
            )?;
            self.log_to_daily_note(&card, note)?;
            drop(_lock);
            self.cascade_unblock(&card.meta.id, &today, &now_ts)?;
            return Ok(card);
        }

        self.write_card(&card)?;
        self.log_event(
            &card.meta.id,
            "pending-close",
            actor,
            card.meta.session_id.as_deref(),
            Some(&prev),
            "pending-close",
            Some(true),
            &card.meta.title,
        )?;
        Ok(card)
    }

    /// Approve a task's current eval for auto-acceptance: when the owner submits and this exact
    /// check passes, the task closes without a manual sign-off. Human-only (`Interactive`).
    pub fn approve_eval(&self, query: &str, approval: Approval) -> Result<TaskCard> {
        if approval == Approval::Untrusted {
            bail!("Only a person at an interactive terminal can approve an eval for auto-accept (`tk auto`)");
        }
        let found = self.find_task(query)?;
        let _lock = Self::lock_card(Path::new(&found.path))?;
        let mut card = Self::read_card(Path::new(&found.path))?;
        card.scope = found.scope;
        if matches!(card.meta.status, TaskStatus::Done | TaskStatus::Archived) {
            bail!("{} is already `{}`", card.meta.id, card.meta.status.as_str());
        }
        let Some(eval) = card.meta.eval.clone() else {
            bail!(
                "{} has no eval to approve. Add one first: `tk set {} --eval '<cmd>'`",
                card.meta.id,
                card.meta.id
            );
        };
        card.meta.approved_eval = Some(eval.clone());
        card.meta.updated = Local::now().format("%Y-%m-%d").to_string();
        card.body = format!(
            "{}\n- `{}`: Auto-accept on: closes when eval `{}` passes (approved by `{}`).",
            card.body.trim_end(),
            Local::now().format("%Y-%m-%d %H:%M"),
            eval,
            self.config.default_owner
        );
        self.write_card(&card)?;
        let st = card.meta.status.as_str().to_string();
        self.log_event(&card.meta.id, "eval-approved", &self.config.default_owner, None, Some(&st), &st, None, &card.meta.title)?;
        Ok(card)
    }

    /// Turn auto-accept off: the task goes back to manual sign-off. Anyone may revoke.
    pub fn revoke_eval_approval(&self, query: &str) -> Result<TaskCard> {
        let found = self.find_task(query)?;
        let _lock = Self::lock_card(Path::new(&found.path))?;
        let mut card = Self::read_card(Path::new(&found.path))?;
        card.scope = found.scope;
        if card.meta.approved_eval.take().is_some() {
            card.body = format!(
                "{}\n- `{}`: Auto-accept off: manual sign-off required.",
                card.body.trim_end(),
                Local::now().format("%Y-%m-%d %H:%M")
            );
            self.write_card(&card)?;
            let st = card.meta.status.as_str().to_string();
            self.log_event(&card.meta.id, "eval-approval-revoked", "system", None, Some(&st), &st, None, &card.meta.title)?;
        }
        Ok(card)
    }

    /// Completes a task as an untrusted caller (agents, MCP, scripts).
    /// Built-in guardrail: this can only ever submit for review, never mark `done`.
    pub fn complete_task(&self, query: &str, actor: &str, note: Option<&str>) -> Result<TaskCard> {
        self.complete_task_with(query, actor, note, Approval::Untrusted)
    }

    /// Completes a task (`status: done`) when `approval` is a human sign-off.
    /// `Untrusted` callers are redirected to `pending-close`, and refused if the task is already there.
    /// Approving `pending-close` work requires the review receipt to still match the output.
    pub fn complete_task_with(
        &self,
        query: &str,
        actor: &str,
        note: Option<&str>,
        approval: Approval,
    ) -> Result<TaskCard> {
        let card_check = self.find_task(query)?;
        let owner = self.config.default_owner.clone();

        if approval == Approval::Untrusted {
            if card_check.meta.status == TaskStatus::PendingClose {
                bail!(
                    "{} is awaiting human sign-off. Only `tk done {}` run by a person at an interactive terminal can close it",
                    card_check.meta.id,
                    card_check.meta.id
                );
            }
            return self.submit_for_review(query, actor, note, None);
        }

        // Agent-owned work goes through review first, even when a human closes it.
        if !card_check.meta.owner.eq_ignore_ascii_case(&owner)
            && card_check.meta.status != TaskStatus::PendingClose
        {
            return self.submit_for_review(query, &card_check.meta.owner, note, None);
        }

        let lock = Self::lock_card(Path::new(&card_check.path))?;
        let mut card = Self::read_card(Path::new(&card_check.path))?;
        card.scope = card_check.scope;
        let prev = card.meta.status.as_str().to_string();

        match card.meta.status {
            TaskStatus::Done | TaskStatus::Archived => {
                bail!("{} is already `{}`", card.meta.id, prev);
            }
            TaskStatus::PendingClose => {
                let receipt = card.meta.receipt.clone().with_context(|| {
                    format!(
                        "{} has no review receipt (submitted before receipts existed). Re-submit with `tk review {}`",
                        card.meta.id, card.meta.id
                    )
                })?;
                if receipt.eval != card.meta.eval {
                    bail!(
                        "Cannot approve {}: the eval command changed after it was submitted. Re-submit with `tk review {}`",
                        card.meta.id,
                        card.meta.id
                    );
                }
                if receipt.snapshot.is_some() && self.worktree_snapshot() != receipt.snapshot {
                    bail!(
                        "Cannot approve {}: the output changed since the eval passed (receipt {}). Re-submit with `tk review {}`",
                        card.meta.id,
                        receipt.snapshot.as_deref().unwrap_or("none"),
                        card.meta.id
                    );
                }
            }
            _ => {
                if self.config.policy.require_eval && card.meta.eval.is_none() {
                    bail!(
                        "Cannot close {}: [policy].require_eval is on and the task has no eval command",
                        card.meta.id
                    );
                }
            }
        }

        let (passed, eval_out) = self.eval_card(&mut card, &owner)?;
        if !passed {
            self.write_card(&card)?;
            bail!(
                "Cannot mark {} as done — Automated Eval FAILED: {}",
                card.meta.id,
                eval_out
            );
        }

        let today = Local::now().format("%Y-%m-%d").to_string();
        let now_ts = Local::now().format("%Y-%m-%d %H:%M").to_string();
        let approval_label = "interactive".to_string();

        card.meta.status = TaskStatus::Done;
        card.meta.updated = today.clone();
        card.meta.approval = Some(approval_label.clone());
        // Check off DoD items in body when completed
        card.body = card.body.replace("- [ ] ", "- [x] ");
        let summary_suffix = match note {
            Some(n) if !n.trim().is_empty() => format!(" — {}", n.trim()),
            _ => String::new(),
        };
        let via = if actor.eq_ignore_ascii_case(&owner) {
            String::new()
        } else {
            format!(" (requested as `{}`)", actor)
        };
        card.body = format!(
            "{}\n- `{}`: Completed (`done`) by `{}`, approval `{}`{} ✅{}.",
            card.body.trim_end(),
            now_ts,
            owner,
            approval_label,
            via,
            summary_suffix
        );

        self.write_card(&card)?;
        self.log_event(
            &card.meta.id,
            "completed",
            &owner,
            card.meta.session_id.as_deref(),
            Some(&prev),
            "done",
            Some(true),
            &card.meta.title,
        )?;
        self.log_to_daily_note(&card, note)?;
        drop(lock);

        self.cascade_unblock(&card.meta.id, &today, &now_ts)?;
        Ok(card)
    }

    /// Promote `blocked` tasks whose blockers are now all closed. Locks each card it rewrites.
    fn cascade_unblock(&self, completed_id: &str, today: &str, now_ts: &str) -> Result<()> {
        let all = self.list_all()?;
        // One status snapshot for the whole cascade: re-reading every card per dependent is quadratic.
        let open: std::collections::HashSet<String> = all
            .iter()
            .filter(|c| !matches!(c.meta.status, TaskStatus::Done | TaskStatus::Archived))
            .map(|c| c.meta.id.to_lowercase())
            .filter(|id| !id.eq_ignore_ascii_case(completed_id))
            .collect();
        for other in all {
            if !other.meta.blocked_by.iter().any(|b| b.eq_ignore_ascii_case(completed_id)) {
                continue;
            }
            let _lock = Self::lock_card(Path::new(&other.path))?;
            let Ok(mut other) = Self::read_card(Path::new(&other.path)).map(|mut c| {
                c.scope = other.scope;
                c
            }) else {
                continue;
            };
            if other.meta.status != TaskStatus::Blocked {
                continue;
            }
            let remaining_blockers = other
                .meta
                .blocked_by
                .iter()
                .filter(|bid| open.contains(&bid.to_lowercase()))
                .count();

            if remaining_blockers == 0 {
                let other_prev = other.meta.status.as_str().to_string();
                other.meta.status = TaskStatus::Ready;
                other.meta.updated = today.to_string();
                other.body = format!(
                    "{}\n- `{}`: Auto-unblocked -> `ready` (blocker `{}` completed).",
                    other.body.trim_end(),
                    now_ts,
                    completed_id
                );
                let _ = self.write_card(&other);
                let _ = self.log_event(
                    &other.meta.id,
                    "unblocked",
                    "system",
                    other.meta.session_id.as_deref(),
                    Some(&other_prev),
                    "ready",
                    None,
                    &other.meta.title,
                );
            }
        }
        Ok(())
    }

    /// Git tree hash of the working tree that evals run in, excluding the task cards themselves
    /// (they change on every transition). Built with a throwaway index, so the user's index and
    /// refs are untouched. `None` outside a git repo, or when the cards are the repo root.
    pub fn worktree_snapshot(&self) -> Option<String> {
        let dir = Path::new(&self.config.vault_path);
        let top = PathBuf::from(git_output(dir, &["rev-parse", "--show-toplevel"], None)?);
        let top_canon = top.canonicalize().ok()?;
        let mut pathspecs = vec![".".to_string()];
        if let Ok(tasks_canon) = self.tasks_path.canonicalize()
            && let Ok(rel) = tasks_canon.strip_prefix(&top_canon)
        {
            if rel.as_os_str().is_empty() {
                return None;
            }
            pathspecs.push(format!(":(exclude){}", rel.display()));
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let index = std::env::temp_dir().join(format!("tkmd-index-{}-{}", std::process::id(), nanos));
        let mut add_args = vec!["add", "-A", "--"];
        add_args.extend(pathspecs.iter().map(String::as_str));
        let tree = git_output(&top, &add_args, Some(index.as_path()))
            .and_then(|_| git_output(&top, &["write-tree"], Some(index.as_path())));
        let _ = fs::remove_file(&index);
        tree.filter(|t| !t.is_empty())
    }

    /// Update properties on a task card (title, priority, owner, due, dod, eval).
    pub fn update_task(
        &self,
        query: &str,
        new_title: Option<String>,
        new_priority: Option<String>,
        new_owner: Option<String>,
        new_due: Option<String>,
        new_dod: Vec<String>,
        new_eval: Option<String>,
    ) -> Result<TaskCard> {
        let mut card = self.find_task(query)?;
        let _lock = Self::lock_card(Path::new(&card.path))?;
        let orig_scope = card.scope;
        card = Self::read_card(Path::new(&card.path))?;
        card.scope = orig_scope;
        let today = Local::now().format("%Y-%m-%d").to_string();
        let now_ts = Local::now().format("%Y-%m-%d %H:%M").to_string();

        let mut changes = Vec::new();

        if let Some(title) = new_title {
            let old_title = card.meta.title.clone();
            card.meta.title = title.clone();
            let old_h1 = format!("# {}", old_title);
            let new_h1 = format!("# {}", title);
            if card.body.contains(&old_h1) {
                card.body = card.body.replace(&old_h1, &new_h1);
            }
            changes.push(format!("title -> '{}'", title));
        }

        if let Some(p) = new_priority {
            let prio = match p.to_lowercase().as_str() {
                "p0" | "(a)" | "a" => "P0",
                "p1" | "(b)" | "b" => "P1",
                "p2" | "(c)" | "c" => "P2",
                _ => "P1",
            };
            card.meta.priority = prio.to_string();
            changes.push(format!("priority -> {}", prio));
        }

        if let Some(o) = new_owner {
            let clean_owner = o.trim().trim_start_matches('@').to_string();
            card.meta.owner = clean_owner.clone();
            changes.push(format!("owner -> @{}", clean_owner));
        }

        if let Some(d) = new_due {
            card.meta.due = Some(d.clone());
            changes.push(format!("due -> {}", d));
        }

        if !new_dod.is_empty() {
            card.meta.dod = new_dod.clone();
            let dod_lines: Vec<String> = new_dod.iter().map(|item| format!("- [ ] {}", item)).collect();
            let new_dod_block = format!("## Definition of Done (DoD)\n{}", dod_lines.join("\n"));
            if let Some(dod_idx) = card.body.find("## Definition of Done (DoD)") {
                let after = &card.body[dod_idx + "## Definition of Done (DoD)".len()..];
                if let Some(next_h2) = after.find("\n## ") {
                    let end_idx = dod_idx + "## Definition of Done (DoD)".len() + next_h2;
                    card.body = format!("{}{}\n{}", &card.body[..dod_idx], new_dod_block, &card.body[end_idx..]);
                } else {
                    card.body = format!("{}{}\n", &card.body[..dod_idx], new_dod_block);
                }
            } else {
                card.body = format!("{}\n\n{}\n", card.body.trim_end(), new_dod_block);
            }
            changes.push(format!("updated {} DoD item(s)", new_dod.len()));
        }

        if let Some(e) = new_eval {
            card.meta.eval = Some(e.clone());
            card.meta.eval_status = None;
            let eval_block = format!("## Automated Eval\n```bash\n{}\n```", e);
            if let Some(eval_idx) = card.body.find("## Automated Eval") {
                let after = &card.body[eval_idx + "## Automated Eval".len()..];
                if let Some(next_h2) = after.find("\n## ") {
                    let end_idx = eval_idx + "## Automated Eval".len() + next_h2;
                    card.body = format!("{}{}\n{}", &card.body[..eval_idx], eval_block, &card.body[end_idx..]);
                } else {
                    card.body = format!("{}{}\n", &card.body[..eval_idx], eval_block);
                }
            } else {
                card.body = format!("{}\n\n{}\n", card.body.trim_end(), eval_block);
            }
            changes.push(format!("eval -> '{}'", e));
            if card.meta.approved_eval.as_deref().is_some_and(|a| a != e) {
                card.meta.approved_eval = None;
                changes.push("auto-accept revoked (eval changed)".to_string());
            }
            // Changing the check invalidates any review built on the old one.
            if card.meta.status == TaskStatus::PendingClose {
                card.meta.status = TaskStatus::InProgress;
                card.meta.receipt = None;
                changes.push("status -> in-progress (review invalidated by eval change)".to_string());
            }
        }

        if changes.is_empty() {
            bail!("No updates specified. Use --title, --priority, --owner, --due, --dod, or --eval");
        }

        card.meta.updated = today;
        let change_summary = changes.join(", ");
        card.body = format!("{}\n- `{}`: Updated properties: {}.", card.body.trim_end(), now_ts, change_summary);

        self.write_card(&card)?;
        Ok(card)
    }

    /// Add dependency: `dependent_id` will be blocked by `blocker_ids`.
    pub fn add_dependency(&self, dependent_id: &str, blocker_ids: &[String]) -> Result<TaskCard> {
        let found = self.find_task(dependent_id)?;
        let today = Local::now().format("%Y-%m-%d").to_string();
        let now_ts = Local::now().format("%Y-%m-%d %H:%M").to_string();

        let mut blockers = Vec::new();
        for raw in blocker_ids {
            let blocker = self.find_task(raw)?;
            if blocker.meta.id.eq_ignore_ascii_case(&found.meta.id) {
                bail!("Task cannot depend on itself: {}", found.meta.id);
            }
            blockers.push(blocker);
        }

        // Refuse edges that would close a cycle: walk each blocker's own dependencies.
        let graph: std::collections::HashMap<String, Vec<String>> = self
            .list_all()?
            .into_iter()
            .map(|c| (c.meta.id.to_lowercase(), c.meta.blocked_by.iter().map(|b| b.to_lowercase()).collect()))
            .collect();
        let target = found.meta.id.to_lowercase();
        for blocker in &blockers {
            let mut stack = vec![(blocker.meta.id.to_lowercase(), vec![blocker.meta.id.clone()])];
            let mut seen = std::collections::HashSet::new();
            while let Some((node, path)) = stack.pop() {
                if node == target {
                    bail!(
                        "Dependency cycle: {} would wait on {}, which already waits on it ({} -> {})",
                        found.meta.id,
                        blocker.meta.id,
                        path.join(" -> "),
                        found.meta.id
                    );
                }
                if !seen.insert(node.clone()) {
                    continue;
                }
                for next in graph.get(&node).into_iter().flatten() {
                    let mut p = path.clone();
                    p.push(next.to_uppercase().replace("TK-", "tk-"));
                    stack.push((next.clone(), p));
                }
            }
        }

        // Update each blocker's `blocks` array under its own lock (one card locked at a time).
        for blocker in &blockers {
            let _lock = Self::lock_card(Path::new(&blocker.path))?;
            let mut blocker_card = Self::read_card(Path::new(&blocker.path))?;
            blocker_card.scope = blocker.scope;
            if !blocker_card.meta.blocks.iter().any(|b| b.eq_ignore_ascii_case(&found.meta.id)) {
                blocker_card.meta.blocks.push(found.meta.id.clone());
                self.write_card(&blocker_card)?;
            }
        }

        let _lock = Self::lock_card(Path::new(&found.path))?;
        let mut dep_card = Self::read_card(Path::new(&found.path))?;
        dep_card.scope = found.scope;
        let mut added = Vec::new();
        for blocker in &blockers {
            if !dep_card.meta.blocked_by.iter().any(|b| b.eq_ignore_ascii_case(&blocker.meta.id)) {
                dep_card.meta.blocked_by.push(blocker.meta.id.clone());
                added.push(blocker.meta.id.clone());
            }
        }

        if !added.is_empty() {
            if dep_card.meta.status == TaskStatus::Ready {
                dep_card.meta.status = TaskStatus::Blocked;
            }
            dep_card.meta.updated = today;
            dep_card.body = format!(
                "{}\n- `{}`: Added dependency on: {}.",
                dep_card.body.trim_end(),
                now_ts,
                added.join(", ")
            );
            self.write_card(&dep_card)?;
        }

        Ok(dep_card)
    }

    pub fn log_event(
        &self,
        id: &str,
        action: &str,
        actor: &str,
        session_id: Option<&str>,
        from_status: Option<&str>,
        to_status: &str,
        eval_passed: Option<bool>,
        title: &str,
    ) -> Result<()> {
        let event = TaskEvent {
            timestamp: Local::now().to_rfc3339(),
            id: id.to_string(),
            action: action.to_string(),
            actor: actor.to_string(),
            session_id: session_id.map(|s| s.to_string()),
            from_status: from_status.map(|s| s.to_string()),
            to_status: to_status.to_string(),
            eval_passed,
            title: title.to_string(),
        };
        let events_file = self.tasks_path.join(".events.jsonl");
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_file)?;
        writeln!(f, "{}", serde_json::to_string(&event)?)?;
        Ok(())
    }

    fn log_to_daily_note(&self, card: &TaskCard, note: Option<&str>) -> Result<()> {
        if card.scope == CardScope::Global && self.scope != CardScope::Global
            && let Some(ref peer) = self.peer {
                return peer.log_to_daily_note(card, note);
            }
        if self.config.mode == "cli" || self.config.daily_dir.trim().is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.daily_path)?;
        let now = Local::now();
        let today_str = now.format("%Y-%m-%d").to_string();
        let daily_file = self.daily_path.join(format!("{}.md", today_str));

        if !daily_file.exists() {
            let iso = now.iso_week();
            let day_name = now.format("%A").to_string();
            let template = format!(
                "---\ntype: daily\nstatus: active\ndate: {today}\nweek: {week:02}\ntags:\n  - daily-notes\n_organized: true\n---\n> [[W{week:02}-{year}]] · {day}\n\n## Notes\n\n\n---\n## Task List\n\n---\n## Earlier Notes\n\n>[!note]-\n> ```dataview\n> LIST\n> FROM \"journals/daily\"\n> WHERE date < this.date\n> SORT date DESC\n> LIMIT 5\n> ```\n\n---\n",
                today = today_str,
                week = iso.week(),
                year = iso.year(),
                day = day_name
            );
            fs::write(&daily_file, template)?;
        }

        let raw = fs::read_to_string(&daily_file)?;
        let stem = Path::new(&card.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&card.meta.id);

        if raw.contains(stem) {
            return Ok(());
        }

        let extra = match note {
            Some(n) if !n.trim().is_empty() => format!(": {}", n.trim()),
            _ => String::new(),
        };
        let task_line = format!(
            "- [x] [[tasks/{stem}|{id}: {title}]] (`@{owner}`){extra} ✅ {today}",
            stem = stem,
            id = card.meta.id,
            title = card.meta.title,
            owner = card.meta.owner,
            extra = extra,
            today = today_str
        );

        let updated = if let Some(idx) = raw.find("## Task List") {
            let after_header = idx + "## Task List".len();
            let next_sep = raw[after_header..]
                .find("\n---")
                .map(|i| after_header + i)
                .unwrap_or(raw.len());
            let section = raw[after_header..next_sep].trim_end();
            format!(
                "{}{}\n{}\n\n{}",
                &raw[..after_header],
                if section.is_empty() { "\n" } else { section },
                task_line,
                raw[next_sep..].trim_start_matches('\n')
            )
        } else {
            format!("{}\n## Task List\n\n{}\n", raw.trim_end(), task_line)
        };

        fs::write(&daily_file, updated)?;
        Ok(())
    }

    fn read_events_from_path(tasks_path: &Path) -> Result<Vec<TaskEvent>> {
        let events_file = tasks_path.join(".events.jsonl");
        if !events_file.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&events_file)?;
        let mut events = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<TaskEvent>(trimmed) {
                events.push(ev);
            }
        }
        Ok(events)
    }

    /// Read all telemetry events from `tasks/.events.jsonl`.
    pub fn read_events(&self) -> Result<Vec<TaskEvent>> {
        let mut events = Self::read_events_from_path(&self.tasks_path)?;
        if self.active_scope == ScopeSelection::All
            && let Some(ref peer) = self.peer {
                let mut peer_events = Self::read_events_from_path(&peer.tasks_path)?;
                events.append(&mut peer_events);
                events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            }
        Ok(events)
    }

    /// Fuzzy search across all task cards (`id`, `title`, `owner`, `project`, `tags`, `dod`, `session_id`, `body`).
    pub fn fuzzy_search(&self, query: &str) -> Result<Vec<TaskCard>> {
        let cards = self.list_all()?;
        let q = query.trim();
        if q.is_empty() {
            return Ok(cards);
        }
        let mut scored: Vec<(i64, TaskCard)> = cards
            .into_iter()
            .filter_map(|c| {
                let searchable = format!(
                    "{} {} @{} {} {} {} {}",
                    c.meta.id,
                    c.meta.title,
                    c.meta.owner,
                    c.meta.project.as_deref().unwrap_or(""),
                    c.meta.tags.join(" "),
                    c.meta.session_id.as_deref().unwrap_or(""),
                    c.body
                );
                // Give a 2x bonus if the match occurs in the primary header (id/title/owner/project/tags)
                let header = format!(
                    "{} {} @{} {} {}",
                    c.meta.id,
                    c.meta.title,
                    c.meta.owner,
                    c.meta.project.as_deref().unwrap_or(""),
                    c.meta.tags.join(" ")
                );
                let score = fuzzy_score(&header, q)
                    .map(|s| s * 2 + 50)
                    .or_else(|| {
                        // In full markdown body, require exact substring match for each token
                        let body_lower = searchable.to_lowercase();
                        let mut body_score = 0i64;
                        for tok in q.split_whitespace() {
                            let pat = tok.to_lowercase();
                            {
                                let pos = body_lower.find(&pat)?;
                                body_score += 100 - (pos.min(80) as i64);
                            }
                        }
                        Some(body_score)
                    })?;
                Some((score, c))
            })
            .collect();
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(scored.into_iter().map(|(_, c)| c).collect())
    }

    /// Archive completed tasks: moves `done` tasks to `tasks/archive/` and sets `status: archived`.
    /// Preserves full `.events.jsonl` history so DuckDB / `tk st` metrics are 100% intact.
    pub fn archive_tasks(&self, id_filter: Option<&str>) -> Result<Vec<String>> {
        let archive_dir = self.tasks_path.join("archive");
        fs::create_dir_all(&archive_dir)?;

        let all_cards = self.list_all()?;
        let mut archived = Vec::new();

        for mut card in all_cards {
            if card.meta.status != TaskStatus::Done {
                continue;
            }
            if let Some(fid) = id_filter
                && !card.meta.id.eq_ignore_ascii_case(fid) {
                    continue;
                }

            let old_path = PathBuf::from(&card.path);
            let filename = old_path.file_name().context("Invalid card filename")?;
            let new_path = archive_dir.join(filename);

            card.meta.status = TaskStatus::Archived;
            card.path = new_path.to_string_lossy().to_string();
            let _ = fs::rename(&old_path, &new_path);
            self.write_card(&card)?;

            let _ = self.log_event(
                &card.meta.id,
                "archived",
                &self.config.default_owner,
                card.meta.session_id.as_deref(),
                Some("done"),
                "archived",
                None,
                &card.meta.title,
            );
            archived.push(card.meta.id);
        }

        Ok(archived)
    }

    /// Prune completed/archived task notes: deletes `.md` cards from disk while keeping `.events.jsonl` database completely intact!
    pub fn prune_tasks(&self, id_filter: Option<&str>) -> Result<Vec<String>> {
        let all_cards = self.list_all()?;
        let mut pruned = Vec::new();

        for card in all_cards {
            if card.meta.status != TaskStatus::Done && card.meta.status != TaskStatus::Archived {
                continue;
            }
            if let Some(fid) = id_filter
                && !card.meta.id.eq_ignore_ascii_case(fid) {
                    continue;
                }

            let path = PathBuf::from(&card.path);
            if path.exists() {
                let _ = fs::remove_file(&path);
            }

            let _ = self.log_event(
                &card.meta.id,
                "pruned",
                &self.config.default_owner,
                card.meta.session_id.as_deref(),
                Some(card.meta.status.as_str()),
                "deleted",
                None,
                &card.meta.title,
            );
            pruned.push(card.meta.id);
        }

        Ok(pruned)
    }
}

/// Fast native Rust fuzzy-matcher (supports space-separated tokens, exact substring bonus,
/// word-boundary acronym jumps, and tight character proximity).
pub fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    let text_lower = text.to_lowercase();
    let mut total_score = 0i64;

    for token in query.split_whitespace() {
        let pat = token.to_lowercase();
        if pat.is_empty() {
            continue;
        }
        // Fast path: exact substring match gets a massive bonus
        if let Some(pos) = text_lower.find(&pat) {
            total_score += 200 - (pos.min(100) as i64);
            continue;
        }
        // Subsequence fuzzy match (e.g. "embscf" matches "EMBA Supply Chain formulas"):
        // Non-consecutive jumps must land on a word boundary or within 3 chars of the previous match.
        let t_chars: Vec<char> = text_lower.chars().collect();
        let p_chars: Vec<char> = pat.chars().collect();
        let mut t_idx = 0usize;
        let mut p_idx = 0usize;
        let mut prev_match_idx: Option<usize> = None;
        let mut token_score = 0i64;

        while t_idx < t_chars.len() && p_idx < p_chars.len() {
            if t_chars[t_idx] == p_chars[p_idx] {
                let is_word_boundary = t_idx == 0
                    || matches!(t_chars[t_idx - 1], ' ' | '-' | '_' | '/' | '[' | '@' | '+');
                let is_consecutive = prev_match_idx == Some(t_idx.saturating_sub(1));

                let allowed = if p_idx == 0 {
                    is_word_boundary
                } else {
                    is_consecutive || is_word_boundary
                };

                if allowed {
                    token_score += 10;
                    if is_consecutive {
                        token_score += 15;
                    }
                    if is_word_boundary {
                        token_score += 20;
                    }
                    prev_match_idx = Some(t_idx);
                    p_idx += 1;
                }
            }
            t_idx += 1;
        }
        if p_idx < p_chars.len() {
            return None; // Token did not match as tight/boundary subsequence
        }
        total_score += token_score;
    }

    Some(total_score)
}

pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    trimmed.chars().take(45).collect::<String>().trim_end_matches('-').to_string()
}

/// Report an unreadable card once per process on stderr (never stdout, which MCP uses).
fn warn_unreadable_card(path: &Path, err: &anyhow::Error) {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    if seen.lock().map(|mut s| s.insert(path.to_path_buf())).unwrap_or(false) {
        let name = path.file_name().and_then(|f| f.to_str()).unwrap_or("?");
        eprintln!("warning: skipped unreadable card {}: {:#}", name, err);
    }
}

/// Run a command, killing it and every process it started if it outlives `timeout`.
/// Returns `Ok(None)` on timeout. stdout and stderr are drained on threads so a chatty eval
/// cannot fill a pipe and stall.
fn run_with_timeout(
    mut cmd: Command,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Read;
    use std::process::Stdio;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut b);
        }
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut b);
        }
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            kill_process_tree(&mut child);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    Ok(status.map(|status| std::process::Output { status, stdout, stderr }))
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        const SIGKILL: i32 = 9;
        // The child leads its own process group, so -pid reaches everything it spawned.
        unsafe { kill(-(child.id() as i32), SIGKILL) };
    }
    let _ = child.kill();
}

/// Run `git -C <dir> <args>`, optionally against a throwaway index. `None` on any failure.
fn git_output(dir: &Path, args: &[&str], index: Option<&Path>) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_task_properties() {
        let tmp = std::env::temp_dir().join(format!("tkmd-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Global,
            peer: None,
            active_scope: ScopeSelection::Global,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };
        fs::create_dir_all(&store.tasks_path).unwrap();

        // 1. Add task
        let card = store
            .add_task("p1 Initial task +test @ken", None, vec![], None, None)
            .unwrap();
        assert_eq!(card.meta.title, "Initial task");
        assert_eq!(card.meta.priority, "P1");

        // 2. Update task with update_task
        let updated = store
            .update_task(
                &card.meta.id,
                Some("Refactored Task".to_string()),
                Some("p0".to_string()),
                Some("claude".to_string()),
                Some("2026-10-25".to_string()),
                vec!["DoD 1".to_string(), "DoD 2".to_string()],
                Some("cargo check".to_string()),
            )
            .unwrap();

        assert_eq!(updated.meta.title, "Refactored Task");
        assert_eq!(updated.meta.priority, "P0");
        assert_eq!(updated.meta.owner, "claude");
        assert_eq!(updated.meta.due.as_deref(), Some("2026-10-25"));
        assert_eq!(updated.meta.dod.len(), 2);
        assert_eq!(updated.meta.eval.as_deref(), Some("cargo check"));
        assert!(updated.body.contains("# Refactored Task"));
        assert!(updated.body.contains("## Definition of Done (DoD)"));
        assert!(updated.body.contains("- [ ] DoD 1"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_concurrent_atomic_writes() {
        let tmp = std::env::temp_dir().join(format!("tkmd-concurrent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let store = std::sync::Arc::new(TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Global,
            peer: None,
            active_scope: ScopeSelection::Global,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        });
        fs::create_dir_all(&store.tasks_path).unwrap();

        let card = store
            .add_task("p1 Shared agent task @ken", None, vec![], None, None)
            .unwrap();

        let mut handles = Vec::new();
        for i in 0..8 {
            let s = store.clone();
            let card_id = card.meta.id.clone();
            let handle = std::thread::spawn(move || {
                let owner = format!("agent_{}", i);
                let _ = s.update_task(
                    &card_id,
                    None,
                    None,
                    Some(owner),
                    None,
                    vec![],
                    None,
                );
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // Re-read card to verify file is valid, non-corrupted Markdown/YAML
        let final_card = store.find_task(&card.meta.id).unwrap();
        assert!(final_card.meta.owner.starts_with("agent_"));
        assert_eq!(final_card.meta.title, "Shared agent task");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_local_vs_global_aggregation() {
        let tmp_global = std::env::temp_dir().join(format!("tkmd-global-{}", std::process::id()));
        let tmp_local = std::env::temp_dir().join(format!("tkmd-local-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_global);
        let _ = fs::remove_dir_all(&tmp_local);
        fs::create_dir_all(tmp_global.join("tasks")).unwrap();
        fs::create_dir_all(tmp_local.join("tasks")).unwrap();

        let cfg_global = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp_global.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };
        let cfg_local = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp_local.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let global_store = TaskStore {
            tasks_path: tmp_global.join("tasks"),
            daily_path: tmp_global.clone(),
            config: cfg_global,
            scope: CardScope::Global,
            peer: None,
            active_scope: ScopeSelection::Global,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };
        let mut local_store = TaskStore {
            tasks_path: tmp_local.join("tasks"),
            daily_path: tmp_local.clone(),
            config: cfg_local,
            scope: CardScope::Local,
            peer: Some(Box::new(global_store)),
            active_scope: ScopeSelection::All,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };

        // Add task to local store
        let c_loc = local_store.add_task("p1 Local repo task", None, vec![], None, None).unwrap();
        assert_eq!(c_loc.scope, CardScope::Local);

        // Add task to peer global store
        let c_glob = local_store.peer.as_ref().unwrap().add_task("p0 Global vault task", None, vec![], None, None).unwrap();
        assert_eq!(c_glob.scope, CardScope::Global);

        // When listing with ScopeSelection::All, should return both!
        let all = local_store.list_all().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|c| c.scope == CardScope::Local && c.meta.title == "Local repo task"));
        assert!(all.iter().any(|c| c.scope == CardScope::Global && c.meta.title == "Global vault task"));

        // When active_scope is Local, returns only local
        local_store.active_scope = ScopeSelection::Local;
        let loc_only = local_store.list_all().unwrap();
        assert_eq!(loc_only.len(), 1);
        assert_eq!(loc_only[0].scope, CardScope::Local);

        let _ = fs::remove_dir_all(&tmp_global);
        let _ = fs::remove_dir_all(&tmp_local);
    }

    #[test]
    fn test_eval_gate_blocks_failing_command() {
        let tmp = std::env::temp_dir().join(format!("tkmd-test-eval-fail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("tasks")).unwrap();

        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };

        // Add task with failing eval command
        let card = store
            .add_task("p0 Failing task", None, vec!["dod item".to_string()], Some("exit 1".to_string()), None)
            .unwrap();
        assert_eq!(card.meta.status, TaskStatus::Ready);

        // Submitting for review must fail!
        let res = store.submit_for_review("1", "agent", Some("Finished work"), None);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Eval failed for tk-001"));

        // Card status must still be ready
        let reloaded = store.find_task("1").unwrap();
        assert_eq!(reloaded.meta.status, TaskStatus::Ready);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_eval_gate_passes_valid_command() {
        let tmp = std::env::temp_dir().join(format!("tkmd-test-eval-pass-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("tasks")).unwrap();

        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };

        // Add task with passing eval command
        store
            .add_task("p0 Passing task", None, vec![], Some("echo 'all tests green' && exit 0".to_string()), None)
            .unwrap();

        // Submitting for review should succeed and transition to pending-close
        let reviewed = store
            .submit_for_review("1", "claude", Some("Ready for Ken"), Some("claude -r 123".to_string()))
            .unwrap();
        assert_eq!(reviewed.meta.status, TaskStatus::PendingClose);
        assert_eq!(reviewed.meta.eval_status.as_deref(), Some("passed"));
        assert!(reviewed.body.contains("PASS ✅"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agent_guardrail_cannot_mark_done() {
        let tmp = std::env::temp_dir().join(format!("tkmd-test-guardrail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("tasks")).unwrap();

        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };

        store.add_task("p1 Agent work", None, vec![], None, None).unwrap();

        // An AI agent attempts to directly complete the task
        let res = store.complete_task("1", "claude", Some("I am done!")).unwrap();

        // Guardrail MUST intercept and force status to pending-close (not done!)
        assert_eq!(res.meta.status, TaskStatus::PendingClose);
        assert_ne!(res.meta.status, TaskStatus::Done);

        // Even if an agent task is called with default_owner ("ken") while in-progress,
        // it must still divert to pending-close first because it has not yet been reviewed!
        let card2 = store.add_task("p1 Another agent task @claude", None, vec![], None, None).unwrap();
        let res2 = store.complete_task(&card2.meta.id, "ken", Some("Agent tries to skip review")).unwrap();
        assert_eq!(res2.meta.status, TaskStatus::PendingClose);
        assert_ne!(res2.meta.status, TaskStatus::Done);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_human_sign_off_marks_done_and_checks_dod() {
        let tmp = std::env::temp_dir().join(format!("tkmd-test-signoff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("tasks")).unwrap();

        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };

        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };

        // Add task with DoD checklist
        let card = store
            .add_task(
                "p0 Deliverable",
                None,
                vec!["Unit tests pass".to_string(), "Docs written".to_string()],
                Some("exit 0".to_string()),
                None,
            )
            .unwrap();
        assert!(card.body.contains("- [ ] Unit tests pass"));

        // Agent submits for review
        store.submit_for_review("1", "agent", Some("All ready"), None).unwrap();

        // Human completes task at an interactive terminal
        let done_card = store
            .complete_task_with("1", "ken", Some("LGTM approved"), Approval::Interactive)
            .unwrap();
        assert_eq!(done_card.meta.status, TaskStatus::Done);
        assert_eq!(done_card.meta.approval.as_deref(), Some("interactive"));
        // DoD items must now be checked off (- [x])
        assert!(done_card.body.contains("- [x] Unit tests pass"));
        assert!(done_card.body.contains("- [x] Docs written"));

        // Verify lifecycle event was written to .events.jsonl
        let events = store.read_events().unwrap();
        assert!(events.iter().any(|e| e.action == "completed" && e.to_status == "done" && e.actor == "ken"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_uninitialized_local_scope_does_not_create_directory() {
        let tmp = std::env::temp_dir().join(format!("tkmd-test-uninit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // Simulate loading an uninitialized local store
        let local_store = TaskStore::load_local_store(&tmp).unwrap();
        assert_eq!(local_store.is_uninitialized_local, false);
        // The tasks directory MUST NOT have been created by load_local_store!
        assert!(!local_store.tasks_path.exists());

        // Listing cards on non-existent directory must return empty Vec cleanly without error
        let cards = local_store.list_store_cards().unwrap();
        assert!(cards.is_empty());
        assert!(!local_store.tasks_path.exists());

        let _ = fs::remove_dir_all(&tmp);
    }
    fn hardening_store(name: &str) -> (TaskStore, PathBuf) {
        let tmp = std::env::temp_dir().join(format!("tkmd-harden-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("tasks")).unwrap();
        let cfg = AppConfig {
            mode: "cli".to_string(),
            vault_path: tmp.to_string_lossy().to_string(),
            tasks_dir: "tasks".to_string(),
            base_file: String::new(),
            daily_dir: String::new(),
            default_owner: "ken".to_string(),
            obsidian_vault_name: None,
            editor: None,
            policy: Policy::default(),
        };
        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
            uninitialized_git_repo: None,
            is_uninitialized_local: false,
        };
        (store, tmp)
    }

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success();
        assert!(ok, "git {:?} failed", args);
    }

    #[cfg(unix)]
    #[test]
    fn test_card_lock_is_held_until_dropped() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (store, tmp) = hardening_store("lock");
        let card = store.add_task("p1 Locked card", None, vec![], None, None).unwrap();
        let path = PathBuf::from(&card.path);

        let held = TaskStore::lock_card(&path).unwrap();
        let acquired = Arc::new(AtomicBool::new(false));
        let flag = acquired.clone();
        let p2 = path.clone();
        let handle = std::thread::spawn(move || {
            let _l = TaskStore::lock_card(&p2).unwrap();
            flag.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!acquired.load(Ordering::SeqCst), "second lock acquired while first was held");
        drop(held);
        handle.join().unwrap();
        assert!(acquired.load(Ordering::SeqCst));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_untrusted_caller_cannot_close_pending_review() {
        let (store, tmp) = hardening_store("untrusted");
        store.add_task("p1 Agent work @claude", None, vec![], Some("exit 0".to_string()), None).unwrap();
        store.submit_for_review("1", "claude", None, None).unwrap();

        // Claiming to be the owner by name is not enough.
        let res = store.complete_task("1", "ken", Some("self-approve"));
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("awaiting human sign-off"));
        assert_eq!(store.find_task("1").unwrap().meta.status, TaskStatus::PendingClose);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_claim_guards_owner_and_state() {
        let (store, tmp) = hardening_store("claim");
        store.add_task("p1 Shared work", None, vec![], None, None).unwrap();
        store.claim_task("1", "claude", None, false).unwrap();

        // Another agent cannot take it silently.
        let steal = store.claim_task("1", "codex", None, false);
        assert!(steal.unwrap_err().to_string().contains("`claude` is working on it"));
        // Re-claiming your own task is fine.
        store.claim_task("1", "claude", None, false).unwrap();
        // Forced takeover is allowed and recorded.
        let taken = store.claim_task("1", "codex", None, true).unwrap();
        assert_eq!(taken.meta.owner, "codex");
        assert!(taken.body.contains("forced takeover from `claude`"));

        // Work under review cannot be reclaimed without --force.
        store.submit_for_review("1", "codex", None, None).unwrap();
        assert!(store.claim_task("1", "claude", None, false).is_err());

        // Closed work can never be reclaimed.
        store.add_task("p1 Mine", None, vec![], None, None).unwrap();
        store.complete_task_with("2", "ken", None, Approval::Interactive).unwrap();
        assert!(store.claim_task("2", "claude", None, true).is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_require_eval_policy_blocks_no_eval_submit() {
        let (mut store, tmp) = hardening_store("require-eval");
        store.config.policy.require_eval = true;
        store.add_task("p1 Unchecked work", None, vec![], None, None).unwrap();
        let res = store.submit_for_review("1", "claude", None, None);
        assert!(res.unwrap_err().to_string().contains("require_eval"));
        assert_eq!(store.find_task("1").unwrap().meta.status, TaskStatus::Ready);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_no_eval_receipt_is_explicit_and_claim_records_eval() {
        let (store, tmp) = hardening_store("no-eval");
        store.add_task("p1 Light task", None, vec![], None, None).unwrap();
        store.claim_task("1", "claude", None, false).unwrap();
        let reviewed = store.submit_for_review("1", "claude", None, None).unwrap();
        let receipt = reviewed.meta.receipt.unwrap();
        assert_eq!(receipt.eval_result, "no-eval");
        assert_eq!(receipt.submitted_by, "claude");
        assert_eq!(reviewed.meta.claimed_eval, None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_eval_change_invalidates_review() {
        let (store, tmp) = hardening_store("eval-change");
        store.add_task("p1 Checked work", None, vec![], Some("exit 0".to_string()), None).unwrap();
        store.submit_for_review("1", "claude", None, None).unwrap();
        let updated = store
            .update_task("1", None, None, None, None, vec![], Some("true".to_string()))
            .unwrap();
        assert_eq!(updated.meta.status, TaskStatus::InProgress);
        assert!(updated.meta.receipt.is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_receipt_goes_stale_when_output_changes() {
        if !git_available() {
            eprintln!("skipping: git not installed");
            return;
        }
        let (store, tmp) = hardening_store("stale");
        git(&tmp, &["init", "-q"]);
        fs::write(tmp.join("work.txt"), "v1").unwrap();
        store.add_task("p1 Edit work file", None, vec![], Some("exit 0".to_string()), None).unwrap();

        let reviewed = store.submit_for_review("1", "claude", None, None).unwrap();
        let snap = reviewed.meta.receipt.as_ref().unwrap().snapshot.clone();
        assert!(snap.is_some(), "expected a git snapshot in a git repo");

        // Card writes must not change the snapshot (cards are excluded).
        assert_eq!(store.worktree_snapshot(), snap);

        // Output changes after the eval passed: approval is refused.
        fs::write(tmp.join("work.txt"), "v2").unwrap();
        let res = store.complete_task_with("1", "ken", None, Approval::Interactive);
        assert!(res.unwrap_err().to_string().contains("output changed"));

        // Re-submitting binds the new output; approval then succeeds.
        store.submit_for_review("1", "claude", None, None).unwrap();
        let done = store.complete_task_with("1", "ken", None, Approval::Interactive).unwrap();
        assert_eq!(done.meta.status, TaskStatus::Done);
        let _ = fs::remove_dir_all(&tmp);
    }

    // ---- Edge cases (TDD: RED marks behaviour the pre-change code got wrong) ----

    /// RED: `find_task` matched the query anywhere in the full path, so a word from a parent
    /// directory selected the first card in the store.
    #[test]
    fn test_find_task_ignores_directory_names() {
        let (store, tmp) = hardening_store("findpath-unique");
        store.add_task("p1 alpha", None, vec![], None, None).unwrap();
        assert!(store.find_task("findpath").is_err(), "directory name matched a card");
        assert!(store.find_task("alpha").is_ok());
        let _ = fs::remove_dir_all(&tmp);
    }

    /// RED: an ambiguous slug query silently picked the first match (dangerous for `tk d`).
    #[test]
    fn test_find_task_ambiguous_query_is_an_error() {
        let (store, tmp) = hardening_store("ambiguous");
        store.add_task("p1 smoke one", None, vec![], None, None).unwrap();
        store.add_task("p1 smoke two", None, vec![], None, None).unwrap();
        let err = store.find_task("smoke").unwrap_err().to_string();
        assert!(err.contains("tk-001") && err.contains("tk-002"), "should list candidates: {}", err);
        assert_eq!(store.find_task("smoke-two").unwrap().meta.id, "tk-002");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// RED: numeric queries fell through to substring path matching, so `100` hit `tk-1000`.
    #[test]
    fn test_numeric_query_never_matches_a_longer_id() {
        let (store, tmp) = hardening_store("ids");
        let first = store.add_task("p1 seed", None, vec![], None, None).unwrap();
        let raw = fs::read_to_string(&first.path).unwrap().replace("id: tk-001", "id: tk-999");
        fs::remove_file(&first.path).unwrap();
        fs::write(store.tasks_path.join("tk-999-seed.md"), raw).unwrap();
        let big = store.add_task("p1 thousandth", None, vec![], None, None).unwrap();
        assert_eq!(big.meta.id, "tk-1000");
        assert_eq!(store.find_task("1000").unwrap().meta.id, "tk-1000");
        assert_eq!(store.find_task("999").unwrap().meta.id, "tk-999");
        assert!(store.find_task("100").is_err(), "`100` must not resolve to tk-1000");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// RED: `add_dependency` accepted cycles, which deadlock the schedule.
    #[test]
    fn test_dependency_cycle_is_rejected() {
        let (store, tmp) = hardening_store("cycle");
        for t in ["p1 a", "p1 b", "p1 c"] {
            store.add_task(t, None, vec![], None, None).unwrap();
        }
        store.add_dependency("2", &["1".to_string()]).unwrap();
        store.add_dependency("3", &["2".to_string()]).unwrap();
        let err = store.add_dependency("1", &["3".to_string()]).unwrap_err().to_string();
        assert!(err.contains("cycle"), "expected a cycle error, got: {}", err);
        assert!(store.find_task("1").unwrap().meta.blocked_by.is_empty(), "cycle edge was written");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// RED: a blocked task (open dependencies) could be submitted for review.
    #[test]
    fn test_blocked_task_cannot_be_submitted() {
        let (store, tmp) = hardening_store("blocked-submit");
        store.add_task("p1 base", None, vec![], None, None).unwrap();
        store.add_task("p1 top needs:1", None, vec![], None, None).unwrap();
        assert_eq!(store.find_task("2").unwrap().meta.status, TaskStatus::Blocked);
        let err = store.submit_for_review("2", "claude", None, None).unwrap_err().to_string();
        assert!(err.contains("tk-001"), "should name the open blocker: {}", err);
        let _ = fs::remove_dir_all(&tmp);
    }

    /// RED: any agent could submit another agent's claimed work.
    #[test]
    fn test_only_the_owner_submits_claimed_work() {
        let (store, tmp) = hardening_store("owner-submit");
        store.add_task("p1 owned", None, vec![], None, None).unwrap();
        store.claim_task("1", "claude", None, false).unwrap();
        let err = store.submit_for_review("1", "codex", None, None).unwrap_err().to_string();
        assert!(err.contains("claude"), "should name the owner: {}", err);
        store.submit_for_review("1", "claude", None, None).unwrap();
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_yaml_in_a_title_is_inert() {
        let (store, tmp) = hardening_store("yaml-title");
        let card = store
            .add_task("p1 innocent\nstatus: done\nowner: mallory", None, vec![], None, None)
            .unwrap();
        let back = TaskStore::read_card(Path::new(&card.path)).unwrap();
        assert_ne!(back.meta.status, TaskStatus::Done);
        assert_ne!(back.meta.owner, "mallory");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_receipt_survives_unrelated_card_changes() {
        if !git_available() {
            return;
        }
        let (store, tmp) = hardening_store("unrelated");
        git(&tmp, &["init", "-q"]);
        fs::write(tmp.join("work.txt"), "v1").unwrap();
        store.add_task("p1 real work", None, vec![], Some("exit 0".to_string()), None).unwrap();
        store.submit_for_review("1", "claude", None, None).unwrap();
        // Other agents keep creating and editing cards while the review waits.
        store.add_task("p1 other work", None, vec![], None, None).unwrap();
        store.claim_task("2", "codex", None, false).unwrap();
        let done = store.complete_task_with("1", "ken", None, Approval::Interactive).unwrap();
        assert_eq!(done.meta.status, TaskStatus::Done);
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Scale: one blocker with many dependents. RED (predicted): the cascade re-read every card
    /// for every dependent (quadratic). Run with `cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn scale_cascade_unblocks_many_dependents_quickly() {
        let (store, tmp) = hardening_store("scale-cascade");
        let n = 1000;
        store.add_task("p1 blocker", None, vec![], None, None).unwrap();
        let base = fs::read_to_string(store.find_task("1").unwrap().path).unwrap();
        for i in 2..=n + 1 {
            let id = format!("tk-{:03}", i);
            let raw = base
                .replace("id: tk-001", &format!("id: {}", id))
                .replace("status: ready", "status: blocked")
                .replacen("---\n", "---\nblocked_by:\n- tk-001\n", 1);
            fs::write(store.tasks_path.join(format!("{}-dep.md", id)), raw).unwrap();
        }
        let start = std::time::Instant::now();
        store.complete_task_with("1", "ken", None, Approval::Interactive).unwrap();
        let took = start.elapsed();
        eprintln!("cascade over {} dependents: {:?}", n, took);
        let ready = store
            .list_store_cards()
            .unwrap()
            .into_iter()
            .filter(|c| c.meta.status == TaskStatus::Ready)
            .count();
        assert_eq!(ready, n);
        assert!(took < std::time::Duration::from_secs(5), "cascade too slow: {:?}", took);
        let _ = fs::remove_dir_all(&tmp);
    }

    // ---- Contract approval: the human approves the check, the agent delivers against it ----

    #[test]
    fn test_auto_accepted_task_closes_when_its_approved_eval_passes() {
        let (store, tmp) = hardening_store("auto-close");
        store.add_task("p1 base", None, vec![], Some("exit 0".to_string()), None).unwrap();
        store.add_task("p1 next needs:1", None, vec![], None, None).unwrap();
        let approved = store.approve_eval("1", Approval::Interactive).unwrap();
        assert_eq!(approved.meta.approved_eval.as_deref(), Some("exit 0"));
        store.claim_task("1", "claude", None, false).unwrap();
        let done = store.submit_for_review("1", "claude", Some("finished"), None).unwrap();
        assert_eq!(done.meta.status, TaskStatus::Done);
        assert_eq!(done.meta.approval.as_deref(), Some("auto:eval"));
        // Auto-closing unblocks dependents like a manual sign-off.
        assert_eq!(store.find_task("2").unwrap().meta.status, TaskStatus::Ready);
        let events = store.read_events().unwrap();
        assert!(events.iter().any(|e| e.action == "auto-accepted" && e.to_status == "done"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agents_cannot_approve_an_eval() {
        let (store, tmp) = hardening_store("auto-untrusted");
        store.add_task("p1 base", None, vec![], Some("exit 0".to_string()), None).unwrap();
        assert!(store.approve_eval("1", Approval::Untrusted).is_err());
        assert!(store.find_task("1").unwrap().meta.approved_eval.is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_changing_the_eval_revokes_auto_accept() {
        let (store, tmp) = hardening_store("auto-revoke");
        store.add_task("p1 base", None, vec![], Some("exit 0".to_string()), None).unwrap();
        store.approve_eval("1", Approval::Interactive).unwrap();
        let changed = store.update_task("1", None, None, None, None, vec![], Some("true".to_string())).unwrap();
        assert!(changed.meta.approved_eval.is_none(), "eval change must revoke the approval");
        let reviewed = store.submit_for_review("1", "claude", None, None).unwrap();
        assert_eq!(reviewed.meta.status, TaskStatus::PendingClose);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_a_failing_approved_eval_does_not_close() {
        let (store, tmp) = hardening_store("auto-fail");
        store.add_task("p1 base", None, vec![], Some("exit 1".to_string()), None).unwrap();
        store.approve_eval("1", Approval::Interactive).unwrap();
        assert!(store.submit_for_review("1", "claude", None, None).is_err());
        assert_eq!(store.find_task("1").unwrap().meta.status, TaskStatus::Ready);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tasks_without_an_eval_cannot_auto_accept() {
        let (store, tmp) = hardening_store("auto-noeval");
        store.add_task("p1 base", None, vec![], None, None).unwrap();
        let err = store.approve_eval("1", Approval::Interactive).unwrap_err().to_string();
        assert!(err.contains("eval"), "{}", err);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_revoking_auto_accept_returns_to_manual_review() {
        let (store, tmp) = hardening_store("auto-off");
        store.add_task("p1 base", None, vec![], Some("exit 0".to_string()), None).unwrap();
        store.approve_eval("1", Approval::Interactive).unwrap();
        store.revoke_eval_approval("1").unwrap();
        let reviewed = store.submit_for_review("1", "claude", None, None).unwrap();
        assert_eq!(reviewed.meta.status, TaskStatus::PendingClose);
        let _ = fs::remove_dir_all(&tmp);
    }
}
