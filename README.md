<p align="center">
  <img src="assets/logo.svg" alt="taskmd logo" width="760">
</p>

<p align="center">
  <b>Markdown Task &amp; Project Manager for Developers and AI Agents</b><br>
  <i>Local plain-text cards • Critical path schedules • Built-in MCP for AI tools</i>
</p>

---

## What is taskmd?

**`taskmd` (`tk`) is a fast, local task manager for developers and AI coding agents.**

It stores every task as a plain Markdown file (`.md`) on your computer. It uses mathematical graph algorithms to schedule work, find blockers, and identify the Critical Path.

When you pair with AI coding agents (such as Claude Code, Codex, or Gemini), `taskmd` prevents agents from falsely claiming work is finished. Agents must run automated test commands (`eval`) and submit work for human review before a task can close.

<p align="center">
  <img src="assets/demo.gif" alt="taskmd interactive terminal recording" width="760">
</p>

### Why Use taskmd?

- **100% Local & Offline**: No cloud accounts, no subscriptions, and zero background daemons. You own your data.
- **AI Agent Guardrails**: AI agents cannot mark tasks as `done`. Agents must submit work to `pending-close` for human sign-off.
- **Mathematical Scheduling**: `taskmd` calculates early start times, buffers, and the Critical Path (`tk dep --cpm`) without guessing.
- **Blocker Detection**: View dependency trees (`tk tree`) and circular deadlocks (`tk dep --dsm`) directly in your terminal.
- **One-Command Session Resume**: Reopen the exact AI terminal session attached to any task (`tk u <id>`).
- **Flexible Storage**: Manage repository-level tasks (`tk init -L`) or central Obsidian Kanban boards (`bases/tasks.base`).

### The 5 Core Pillars (MD)

1. **Markdown**: Plain-text files you own. Edit cards with any text editor (`hx`, `nvim`, `micro`, `vim`).
2. **Managing Director**: Executive sign-off gates (`pending-close`) and one-keystroke agent session resumption (`tk u`).
3. **Model-Driven**: Native Model Context Protocol server (`tk mcp`) for Claude, Codex, Gemini, and Cursor.
4. **Mission Delivery**: Automated verification test commands (`eval`) that must pass before review.
5. **Matrix & Dependencies**: Exact Critical Path Method (CPM) schedules and Dependency Structure Matrices (DSM).

It supports two first-class local setups:
1. **Obsidian + CLI Mode (Recommended)** — Syncs 1:1 with an Obsidian `.base` Kanban board (`bases/tasks.base`), updates `.obsidian/types.json`, and logs completed tasks under `## Task List` in your Daily Note.
2. **Standalone CLI Mode** — Pure plain-text Markdown cards in a local directory (`~/.local/share/taskmd/tasks` or `~/tasks`), opening directly in `$EDITOR` (`hx`, `nvim`, `micro`, `vim`) with zero Obsidian dependency.

### 100% Local & Offline Privacy
- **Zero Network**: `tk` makes zero network/HTTP requests and has zero telemetry beacons. No accounts, no cloud sync, no tracking.
- **Plain-Text Data Ownership**: Every task is an open `.md` file; every event is an open `.jsonl` record on your local disk.
- **Self-Contained Telemetry**: `tk st` computes Little's Law, MTTC, and TPM90 directly in Rust. DuckDB is **not required** to run `tk`, but because the event log is standard `.jsonl`, users can optionally run instant SQL queries with DuckDB (`duckdb -c "SELECT * FROM 'tasks/.events.jsonl'"`).

---

## 1. Install `tk`

### Option A: Standalone Binary (No Rust Required)

```bash
curl -fsSL https://raw.githubusercontent.com/ktreharrison/taskmd/main/install.sh | sh
```

### Option B: Build from Source with Cargo (Rust)

```bash
# Install directly from git:
cargo install --git https://github.com/ktreharrison/taskmd.git

# Or build locally from cloned repository:
cargo install --path .
```

Verify the binary is on your `$PATH`:

```bash
tk --help
```

---

## 2. Local Setup (`tk init`)

You can run the interactive setup wizard at any time:

```bash
tk init
```

Or initialize non-interactively with flags:

### Option A: Obsidian 1.14 + CLI (Recommended)

```bash
tk init --obsidian --path ~/Vault --base bases/tasks.base --daily journals/daily
```

What this configures automatically:
- Creates `<vault>/tasks/` (e.g. `~/Vault/tasks/`) for atomic Markdown task cards (`tk-001-*.md`).
- Creates `<vault>/bases/tasks.base` with 5 pre-configured Obsidian 1.14 Base views:
  1. **Kanban Board** (grouped by `status`: `ready`, `in-progress`, `pending-close`, `blocked`, `done`)
  2. **Pending Close (Sign-Off Queue)** (`status == "pending-close"`)
  3. **Your Personal Queue** (`owner == "<you>"`)
  4. **Agent Queues** (`owner != "<you>"`)
  5. **Completed Tasks** (`status == "done"`)
- Safely merges task property types (`id`, `status`, `priority`, `owner`, `project`, `due`, `dod`, `eval`, `eval_status`) into `.obsidian/types.json` without overwriting your existing vault properties.
- Writes `~/.config/taskmd/config.toml` so `tk o` opens `bases/tasks.base` directly in Obsidian.

### Option B: Standalone CLI Mode (No Obsidian Required)

```bash
tk init --cli --path ~/.local/share/taskmd
```

What this configures automatically:
- Creates `~/.local/share/taskmd/tasks/` for Markdown task cards and `.events.jsonl`.
- Disables Obsidian URI calls and Daily Note mutations (`mode = "cli"`).
- Routes `tk o` (`tk open`) and `tk e <id>` (`tk edit`) directly to your `$EDITOR` (defaults to Helix `hx`).

### Option C: Local Repository / Project Backlog (`tk init -L`)

```bash
tk init -L
```

What this configures automatically:
- Writes `.taskmd.toml` in your current repository root.
- Creates `./tasks/` and `./tasks/.events.jsonl` local to the project.
- Automatically detected by `tk` commands in that directory or its subdirectories.
- Retains instant access to the central global vault via `-G` or merged view via `-A`.

---

## 3. Scope Control (Local Project vs. Global Vault)

`taskmd` seamlessly manages both local project-level backlogs and central global backlogs:

| Flag | Scope | Description |
| :--- | :--- | :--- |
| *(default)* | Auto | Uses `./tasks` if inside a project with `.taskmd.toml` or `tasks/`; otherwise uses global vault |
| `-L, --local` | Local | Force operation strictly on the current repository's `./tasks` backlog |
| `-G, --global` | Global | Force operation strictly on the global central vault (`~/Documents/tasks`) |
| `-A, --all-scopes` | Merged | Aggregate local and global tasks across dashboards, CPM schedules, and telemetry |

In merged view (`tk -A`), task IDs display distinct scope indicators (`tk-001L` for local, `tk-001G` for global) without column drift. Mutative commands (`tk d`, `tk c`, `tk r`) target the exact card on disk regardless of active working directory.

## 4. Daily Ergonomics (1-Letter Shortcuts)

| Command | Shortcut | Action |
| :--- | :--- | :--- |
| `tk` | `tk` | Render the responsive terminal dashboard + `pending-close` review banner |
| `tk watch` | `tk w` | Live auto-resizing dashboard (reflows on pane resize or file change) |
| `tk review` | `tk r` | Dedicated sign-off dashboard displaying pending local and global tasks |
| `tk review 1 -n "All checks green"` | `tk r 1` | Move task to `pending-close` (auto-runs `eval` gate + macOS notification) |
| `tk done 1 -n "Verified"` | `tk d 1` | Sign off and close task (`status: done` + Daily Note entry in Obsidian mode) |
| `tk add p1 Fix auth +app @ken due:tomorrow` | `tk a ...` | Create a task card using `todo.txt` shorthand (`p0`/`p1`/`p2`, `+project`, `sid:<id>`) |
| `tk claim 1 --owner claude -s 8y9380573586` | `tk c 1` | Claim `tk-001` (`status: in-progress`) and attach agent `session_id` |
| `tk resume 1` | `tk u 1` | Resume the AI agent session attached to `tk-001` (`claude -r <id>`, `codex resume <id>`, etc.) |
| `tk eval 1` | `tk eval 1` | Execute the card's `eval` shell command and log `[PASS]` / `[FAIL]` |
| `tk find [query]` / `tk find -i` | `tk f` | Fuzzy search tasks (inline table or interactive `fzf` picker + `bat` preview) |
| `tk history [id]` | `tk h` | Event audit log from `tasks/.events.jsonl` with step duration deltas |
| `tk stats` | `tk st` | Flow telemetry: Little's Law, MTTC, P50/P90/TPM90, agent split, yield |
| `tk tree` | `tk tree` | Render dependency DAG tree for active tasks (also `tk dep tree`) |
| `tk dep [id] --needs [ids]` | `tk dep` | Dependencies: CPM table (`--cpm`), text summaries (`--short`, `--long`, `--summary`), colored JSON (`--json`), tree, DSM (`--dsm`), Gantt (`--gantt`), AON (`--aon`) |
| `tk archive [id]` | `tk archive` | Move completed tasks to `tasks/archive/` (preserves `.events.jsonl` DB) |
| `tk prune [id] [-f]` | `tk prune` | Remove completed/archived cards from disk (preserves `.events.jsonl` DB) |
| `tk set 1 -t "Title" -p p0` | `tk set 1` | Update title, priority, owner, due, DoD, or eval directly from CLI |
| `tk edit 1` | `tk e 1` | Edit the Markdown card in `$EDITOR` (`hx`) |
| `tk open` / `tk open 1` | `tk o` / `tk o 1` | Open `bases/tasks.base` (or `tk-001`) in Obsidian (or `$EDITOR` in CLI mode) |

---

## 5. Connect AI Agents via MCP (`tk mcp`)

`tk mcp` runs a dual-framing JSON-RPC 2.0 MCP server over `stdio` (auto-detecting both newline-delimited JSON and `Content-Length` headers for universal compatibility across Claude Desktop, Codex, Gemini CLI, Cursor, Zed, OpenCode, and Pi).

Add this to your agent's MCP configuration:

```json
{
  "mcpServers": {
    "taskmd": {
      "command": "tk",
      "args": ["mcp"]
    }
  }
}
```

### Bundled Agent Skill (`skills/taskmd`)

`taskmd` ships with a pre-configured agent skill in `skills/taskmd/SKILL.md` (and `skills/taskmd/agents/openai.yaml`).

Install it into your agent workspace:

```bash
# For Codex, Claude, Antigravity, OpenCode, Pi:
mkdir -p ~/.agents/skills
cp -r skills/taskmd ~/.agents/skills/
```

This enforces the task governance harness across all your agents:
- Automatic session registration (`-s <session_id>`) for instant resumption (`tk u <id>`).
- Mandatory review gate: agents must submit completed tasks to `pending-close` (`tk r`) instead of `done`.
- Automated eval assertion passes before review submission (`tk eval`).
- DoD deliverable note links (`[[wikilinks]]`).

---

## 5. Event Stream (`tasks/.events.jsonl`) & Optional DuckDB Queries

`tk` records every state transition, eval verification, and agent session in an append-only, newline-delimited JSON stream (`tasks/.events.jsonl`).

`tk st` and `tk h` parse this file natively without any external database. However, because it is standard `.jsonl`, users with `duckdb` installed can run instant SQL analytics with zero setup or database importing:

```bash
# Recent event transitions
duckdb -c "SELECT timestamp, id, action, actor, from_status, to_status, eval_passed FROM read_json_auto('~/Documents/tasks/.events.jsonl') ORDER BY timestamp DESC;"

# Agent review yield
duckdb -c "SELECT actor, count(*) as reviews, count(CASE WHEN eval_passed THEN 1 END) as passed FROM read_json_auto('~/Documents/tasks/.events.jsonl') WHERE action = 'pending-close' GROUP BY actor;"
```

---

## 6. License

`taskmd` is dual-licensed under:

- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))

You may choose either license at your option. Copyright &copy; 2026 Ken Harrison.

