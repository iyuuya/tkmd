# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2] - 2026-10-20

### Added
- **ANSI Colorized Help Output**: Vibrant CLI styling for `-h` and `--help` with bold green headers, bold cyan subcommands/flags, and cyan placeholders. Respects `NO_COLOR` spec.

### Fixed
- **Accurate Review Diversion Message**: `tk done` reports "Submitted for review" instead of "Completed" when an AI agent is intercepted by review guardrails to `pending-close`.

## [0.1.1] - 2026-10-19

### Fixed
- Updated visible `taskmd` strings to `TKMD` across telemetry outputs and Gantt chart headers.

## [0.1.0] - 2026-10-18

### Added
- **Markdown-Native Card Store**: Atomic task cards (`tasks/tk-XXX-*.md`) with YAML frontmatter and Markdown body.
- **Obsidian 1.14 Kanban Base**: Auto-generated `bases/tasks.base` with 5 views (Kanban, Pending Close, Personal Queue, Agent Queue, Completed Tasks) and safe `.obsidian/types.json` merging.
- **Append-Only Event Stream**: `tasks/.events.jsonl` logging every state transition, eval check, and agent session handoff.
- **Flow Telemetry (`tk st`)**:
  - Little's Law engine ($L = \lambda W$) calculating active WIP, cycle times, and implied system throughput.
  - Mean Time to Complete (MTTC / Lead Time) with Mean, P50, P90, and TPM90 (Trimmed Percentile Mean 90).
  - Queue wait time, execution duration, and sign-off wait duration.
  - Review yield and agent execution splits.
- **Dependency Management (`tk dep`)**:
  - Declaring dependencies via `tk dep <id> --needs <ids>` or inline `needs:`, `after:`, `dep:`.
  - Claim guardrail preventing work on blocked tasks until prerequisites finish.
  - Cascade unblocking automatically promoting downstream tasks to `ready` with macOS notifications.
  - ASCII DAG dependency tree rendering (`tk dep tree`).
- **Lifecycle Cleanup (`tk archive` & `tk prune`)**:
  - `tk archive` moves closed task cards to `tasks/archive/`.
  - `tk prune [-f]` purges finished cards from disk while preserving 100% of telemetry in `.events.jsonl`.
- **Property Editing (`tk set`)**:
  - CLI command `tk set` (alias `tk update`) to update title, priority, owner, due date, DoD, and eval shell command without opening an editor.
- **Automated Verification Gates (`tk eval`)**:
  - Configurable shell commands executed in the project workspace, logging exit status and output.
- **Agent Governance & Guardrails**:
  - AI agents calling `tk done` or MCP `complete_task` are automatically routed to `pending-close` for human sign-off.
- **Multi-Agent Session Resumption (`tk u`)**:
  - Session tracking across `agy`, `claude`, `codex`, `hermes`, `opencode`, `pi`, and `gemini`.
  - Auto-generated `## Agent Sessions` markdown table with copy-pasteable resume and continue commands.
- **Dual-Framing stdio MCP Server (`tk mcp`)**:
  - Auto-detects newline-delimited JSON and `Content-Length` header framing for universal AI assistant compatibility.
- **Cross-Platform Releases & Zero-Rust Installer**:
  - Automated GitHub Actions build matrix for macOS (arm64, x86_64), Linux (x86_64), and Windows (x86_64).
  - Single-command shell installer: `curl -fsSL .../install.sh | sh`.
