---
name: taskmd
description: Manage Human + AI Agent tasks using the `tk` CLI or `taskmd` MCP server (`tasks/*.md`, `bases/tasks.base`, and `tasks/.events.jsonl`). Use whenever listing, creating, claiming, attaching agent session IDs, evaluating, submitting for review (`pending-close`), or completing tasks.
---

# `taskmd` (`tk`) — Human + Agent Task Orchestrator

All active tasks live as atomic Markdown cards in `tasks/tk-XXX-*.md` (and render on the native Obsidian 1.14 Kanban board at `bases/tasks.base` when Obsidian mode is enabled).

## Mandatory Agent Protocol (Harness Engineering)

1. **Discover work**:
   ```bash
   tk ls
   tk ls --owner <agent-name> --status ready
   ```
2. **Create a task with Definition of Done (`--dod`), Automated Eval (`--eval`), and Session ID (`-s`)**:
   - **MANDATORY**: Where a DoD criteria produces or references a deliverable note, artifact, or document, agents **MUST** format it as an Obsidian clickable `[[wikilink]]` (e.g. `[[knowledge/auth-spec|Auth specification note]]`).
   ```bash
   tk a p1 Fix auth flow +agent-os @claude due:tomorrow \
     --dod "[[specs/auth-spec|Architecture specification approved]]" \
     --dod "Unit tests pass in CI" \
     --eval "cargo test" \
     -s "<your-session-id>"
   ```
3. **Claim a task & attach your Session ID (`status: in-progress`)**:
   Always pass your agent session ID (`-s <session_id>`, or full resume command like `-s "claude -r 8y9380573586"`) so the user can jump straight back into your conversation with `tk u <id>`:
   ```bash
   tk c 1 -o <agent-name> -s "<session-id>"
   ```
4. **Run automated verification (`eval`)**:
   ```bash
   tk eval 1 -a <agent-name>
   ```
5. **Submit for Sign-Off (`status: pending-close`)**:
   - **CRITICAL RULE**: AI agents **MUST NOT** mark tasks as `done` (`tk d`). Agents **MUST** submit finished work to `pending-close` using `tk r` (`tk review`) once `tk eval` passes.
   - Even if an agent calls `tk d --actor <agent>` or MCP `task_complete`, `tk`'s built-in guardrail hook automatically intercepts non-human actors and routes the task to `pending-close`.
   ```bash
   tk r 1 -a <agent-name> -s "<session-id>" -n "Summary of changes and [[artifact-wikilink]]"
   ```
6. **Resume an Agent Session (`tk u <id>`)**:
   ```bash
   tk u 1        # Executes `claude -r <sid>`, `codex resume <sid>`, `hermes -r <sid>`, etc.
   tk u 1 -p     # Prints the resume command string
   ```
7. **Manage Dependencies & Critical Path (`tk dep`)**:
   ```bash
   tk dep 3 --needs 1,2   # Blocks tk-003 until tk-001 and tk-002 are done
   tk dep --cpm           # Plain-text Critical Path table (Dur, ES, EF, LS, LF, Slack, Condition)
   tk dep summary         # Plain-text STE-100 full technical schedule summary
   tk dep short           # Concise 1-paragraph summary (or tk dep summary --short)
   tk dep long            # Full multi-section breakdown (or tk dep --cpm --long)
   tk dep --cpm --json    # Colored JSON in terminal (raw for pipes), contains summary_short and summary_long
   tk dep tree            # Visualizes dependency DAG tree of active blocked tasks
   tk dep --dsm           # Renders Design Structure Matrix (N x N dependency check)
   tk dep --gantt         # Outputs Mermaid Gantt schedule chart
   tk dep --aon           # Outputs Activity-on-Node (AON) CPM network diagram
   tk dep --mermaid       # Outputs standard Mermaid flowchart
   ```
   - **MCP Tool `task_cpm`**: AI agents can query project duration, critical path sequences, and task slack directly without diagrams or shell execution.
   - In `todo.txt` syntax, tasks can declare dependencies with `needs:1,2`, `after:1,2`, or `dep:1,2`.
   - Blocked tasks cannot be claimed (`claim` guardrail).
   - Completing a blocker (`tk d`) automatically triggers a cascade check, promoting newly unblocked tasks from `blocked` to `ready` and sending a notification.
8. **Archive and Prune (`tk archive` & `tk prune`)**:
   - Deliverable notes created as the Definition of Done (linked via `[[wikilink]]`) remain permanent in the user's workspace.
   - Task cards themselves can be archived or cleaned up once closed:
     ```bash
     tk archive 1        # Moves tk-001.md to tasks/archive/ and sets status: archived
     tk prune 1 -f       # Deletes tk-001.md from active disk
     ```
   - **Database Preservation**: Pruning `.md` cards does **not** erase project history. The append-only event log (`tasks/.events.jsonl`) is strictly preserved, retaining 100% of telemetry, audit history, and Little's Law / MTTC / TPM90 analytics (`tk st`).
9. **Edit Task Properties or Body (`tk set` / `tk edit` / Obsidian)**:
   - CLI property update:
     ```bash
     tk set 1 -t "New Title" -p p0 -o @claude --due 2026-10-25 --dod "[[new-dod]]"
     ```
   - Interactive editor:
     ```bash
     tk edit 1       # Opens markdown card in $EDITOR (hx, nvim, micro, code)
     ```
   - Obsidian visual UI:
     ```bash
     tk open 1       # Opens markdown card directly in Obsidian
     ```

## Hooks (`tk hook`)

- **SessionStart / UserPromptSubmit Hook**:
  ```bash
  tk hook status
  ```
  Injects a compact list of active `ready`, `in-progress`, and `pending-close` tasks + resume commands.
- **Stop Hook**:
  ```bash
  tk hook stop --owner <agent-name>
  ```
  Warns the agent before exiting if it still has an `in-progress` task that hasn't been evaluated (`tk eval`) and submitted (`tk r`).
