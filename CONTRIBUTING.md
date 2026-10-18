# Contributing to `tkmd` (`tk`)

Thank you for your interest in contributing to `tkmd`!

`tkmd` is designed to be a high-performance, single-binary, local-first bridge between human task management (Obsidian / CLI) and autonomous AI coding agents (`agy`, `claude`, `codex`, `hermes`, etc.).

---

## Core Invariants

All contributions must preserve these architectural invariants:

1. **100% Local-First & Air-Gapped**: Zero network requests, zero telemetry beacons, zero cloud accounts.
2. **Zero Runtime Dependencies**: The compiled binary (`tkmd` / `tk`) must remain a self-contained static executable with no external interpreter dependencies (no Node, Python, SQLite, or C shared libraries).
3. **Plain-Text Data Ownership**: All state lives in standard Markdown (`.md`) files and newline-delimited JSON (`.events.jsonl`).
4. **Harness Engineering**: Autonomous AI agents must never mark tasks as `done` directly. Agent completion requests must always route through the `pending-close` review gate.

---

## Project Structure

```text
src/
├── model.rs       # TaskFrontmatter, TaskStatus, EventEntry data models
├── store.rs       # Filesystem store, YAML serialization, cascade unblocking, archiving
├── analytics.rs   # Little's Law (L = λW), MTTC, P50/P90, and TPM90 flow telemetry
├── todotxt.rs     # todo.txt CLI shorthand parser (p0/p1/p2, +proj, @owner, needs:1,2)
├── mcp.rs         # Dual-framing stdio JSON-RPC 2.0 MCP server for AI agents
└── main.rs        # Clap CLI definition, terminal UI dashboard, and subcommand routing
```

---

## Development Setup

### Prerequisites
- [Rust](https://rustup.rs/) (edition 2024 / Rust 1.80+)

### Building and Testing

```bash
# Clone the repository
git clone https://github.com/ktreharrison/tkmd.git
cd tkmd

# Run test suite
cargo test

# Build debug binary
cargo build

# Build optimized release binary
cargo build --release

# Run linter and formatter checks
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

---

## Pull Request Guidelines

1. **Keep it focused**: One bug fix or feature per pull request.
2. **Add unit tests**: Parser changes (`todotxt.rs`) and analytics calculations (`analytics.rs`) must include automated unit tests.
3. **Update documentation**: When adding or changing CLI flags or MCP tools, update `README.md`, `CHANGELOG.md`, and `SKILL.md`.
4. **Preserve formatting**: Run `cargo fmt` before submitting.
