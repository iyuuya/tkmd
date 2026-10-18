use crate::store::TaskStore;
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};

/// Runs a Model Context Protocol (MCP) JSON-RPC 2.0 server over stdio.
/// Supports BOTH newline-delimited JSON-RPC and LSP-style `Content-Length:` framed JSON-RPC
/// so every agent (Claude Code, Codex, Pi, OpenCode, Goose, Gemini, Cursor) works out of the box.
pub fn run_mcp_server(store: &TaskStore) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout();

    loop {
        let mut first_line = String::new();
        let bytes_read = reader.read_line(&mut first_line)?;
        if bytes_read == 0 {
            break; // EOF
        }

        let trimmed = first_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Auto-detect LSP Content-Length framing vs newline-delimited JSON
        let (payload, use_content_length) =
            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                let len: usize = len_str.trim().parse().unwrap_or(0);
                // Consume remaining headers until empty line
                loop {
                    let mut header_line = String::new();
                    if reader.read_line(&mut header_line)? == 0 || header_line.trim().is_empty() {
                        break;
                    }
                }
                let mut buf = vec![0u8; len];
                reader.read_exact(&mut buf)?;
                (String::from_utf8_lossy(&buf).to_string(), true)
            } else {
                (trimmed.to_string(), false)
            };

        let req: Value = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

        if id.is_none() {
            continue;
        }

        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "tkmd",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
            "ping" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "task_list",
                            "description": "List tasks across local project repository and/or central global vault with optional status, owner, or scope filter.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "owner": { "type": "string", "description": "Filter by owner (e.g. ken, hermes, pi, codex)" },
                                    "status": { "type": "string", "description": "Filter by status (inbox, ready, in-progress, pending-close, blocked, done)" },
                                    "scope": { "type": "string", "enum": ["local", "global", "all"], "description": "Filter by task scope: 'local' (repository project backlog), 'global' (central vault backlog), or 'all' (merged)" }
                                }
                            }
                        },
                        {
                            "name": "task_add",
                            "description": "Create a new task with todo.txt shorthand, Definition of Done (dod), optional shell eval command, and optional agent session_id.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "shorthand": { "type": "string", "description": "todo.txt formatted string" },
                                    "details": { "type": "string", "description": "Optional markdown description" },
                                    "dod": { "type": "array", "items": { "type": "string" }, "description": "Definition of Done checklist items" },
                                    "eval": { "type": "string", "description": "Shell command to verify completion (exit 0 = pass)" },
                                    "session_id": { "type": "string", "description": "Agent session ID or full resume command (e.g. '8y9380573586' or 'claude -r 8y9380573586')" }
                                },
                                "required": ["shorthand"]
                            }
                        },
                        {
                            "name": "task_claim",
                            "description": "Claim a task, attach your agent session_id for easy resume, and transition status to in-progress.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Task ID (e.g. tk-001)" },
                                    "owner": { "type": "string", "description": "Agent or human claiming the task (e.g. claude, codex, hermes, pi)" },
                                    "session_id": { "type": "string", "description": "Agent session ID or resume command (e.g. '8y9380573586' or 'claude -r 8y9380573586')" }
                                },
                                "required": ["id", "owner"]
                            }
                        },
                        {
                            "name": "task_eval",
                            "description": "Run the automated verification (eval) command for a task and record pass/fail.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Task ID (e.g. tk-001)" },
                                    "actor": { "type": "string", "description": "Agent running the eval" }
                                },
                                "required": ["id"]
                            }
                        },
                        {
                            "name": "task_submit_review",
                            "description": "Run eval and transition task to pending-close for Ken's final sign-off.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Task ID (e.g. tk-001)" },
                                    "actor": { "type": "string", "description": "Agent submitting the task" },
                                    "note": { "type": "string", "description": "Summary of work or [[wikilink]] to artifact" },
                                    "session_id": { "type": "string", "description": "Agent session ID or resume command (e.g. '8y9380573586' or 'claude -r 8y9380573586')" }
                                },
                                "required": ["id"]
                            }
                        },
                        {
                            "name": "task_complete",
                            "description": "Verify eval passes, mark task as done, and log completion to today's Obsidian Daily Note.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Task ID (e.g. tk-001)" },
                                    "actor": { "type": "string", "description": "Who completed the task" },
                                    "note": { "type": "string", "description": "Optional summary or [[wikilink]] to output artifact" }
                                },
                                "required": ["id"]
                            }
                        },
                        {
                            "name": "task_cpm",
                            "description": "Calculate Critical Path Method (CPM) metrics (ES, EF, LS, LF, Slack, project duration, critical path) for active tasks without diagrams.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        }
                    ]
                }
            }),
            "tools/call" => {
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let tool_res = handle_tool_call(store, tool_name, &args);
                match tool_res {
                    Ok(text) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": text }]
                        }
                    }),
                    Err(err) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "isError": true,
                            "content": [{ "type": "text", "text": format!("Error: {}", err) }]
                        }
                    }),
                }
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }),
        };

        let out_str = serde_json::to_string(&response)?;
        if use_content_length {
            write!(stdout, "Content-Length: {}\r\n\r\n{}", out_str.len(), out_str)?;
        } else {
            writeln!(stdout, "{}", out_str)?;
        }
        stdout.flush()?;
    }

    Ok(())
}

fn handle_tool_call(store: &TaskStore, name: &str, args: &Value) -> Result<String> {
    match name {
        "task_list" => {
            let owner_filter = args.get("owner").and_then(|v| v.as_str());
            let status_filter = args.get("status").and_then(|v| v.as_str());
            let scope_filter = args.get("scope").and_then(|v| v.as_str());
            let cards = store.list_all()?;
            let filtered: Vec<_> = cards
                .into_iter()
                .filter(|c| {
                    let owner_ok = owner_filter.is_none_or(|o| c.meta.owner.eq_ignore_ascii_case(o));
                    let status_ok = status_filter
                        .is_none_or(|s| c.meta.status.as_str().eq_ignore_ascii_case(s));
                    let scope_ok = scope_filter.is_none_or(|s| match s.to_lowercase().as_str() {
                        "local" => c.scope == crate::model::CardScope::Local,
                        "global" => c.scope == crate::model::CardScope::Global,
                        _ => true,
                    });
                    owner_ok && status_ok && scope_ok
                })
                .collect();
            Ok(serde_json::to_string_pretty(&filtered)?)
        }
        "task_add" => {
            let shorthand = args
                .get("shorthand")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let details = args.get("details").and_then(|v| v.as_str());
            let dod = args
                .get("dod")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let eval = args
                .get("eval")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let card = store.add_task(shorthand, details, dod, eval, session_id)?;
            Ok(serde_json::to_string_pretty(&card)?)
        }
        "task_claim" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let owner = args
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let card = store.claim_task(id, owner, session_id)?;
            Ok(serde_json::to_string_pretty(&card)?)
        }
        "task_eval" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let actor = args
                .get("actor")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            let (card, passed, output) = store.run_eval(id, actor)?;
            Ok(serde_json::to_string_pretty(&json!({
                "id": card.meta.id,
                "passed": passed,
                "output": output,
                "card": card
            }))?)
        }
        "task_submit_review" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let actor = args
                .get("actor")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            let note = args.get("note").and_then(|v| v.as_str());
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let card = store.submit_for_review(id, actor, note, session_id)?;
            Ok(serde_json::to_string_pretty(&card)?)
        }
        "task_complete" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let actor = args
                .get("actor")
                .and_then(|v| v.as_str())
                .unwrap_or(&store.config.default_owner);
            let note = args.get("note").and_then(|v| v.as_str());
            let card = store.complete_task(id, actor, note)?;
            Ok(serde_json::to_string_pretty(&card)?)
        }
        "task_cpm" => {
            let cards = store.list_all()?;
            let cpm = crate::analytics::compute_cpm(&cards);
            Ok(serde_json::to_string_pretty(&cpm)?)
        }
        _ => anyhow::bail!("Unknown MCP tool: {}", name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CardScope;
    use crate::store::{AppConfig, ScopeSelection, TaskStore};
    use std::fs;

    fn setup_test_store(name: &str) -> (TaskStore, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("taskmd-mcp-test-{}-{}", name, std::process::id()));
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
        };

        let store = TaskStore {
            tasks_path: tmp.join("tasks"),
            daily_path: tmp.clone(),
            config: cfg,
            scope: CardScope::Local,
            peer: None,
            active_scope: ScopeSelection::Local,
        };
        (store, tmp)
    }

    #[test]
    fn test_mcp_tool_lifecycle() {
        let (store, tmp) = setup_test_store("lifecycle");

        // 1. Add task via MCP
        let add_args = json!({
            "shorthand": "p0 Test MCP tool integration +ci dur:1h",
            "details": "Integration test for MCP",
            "dod": ["Item 1", "Item 2"],
            "eval": "exit 0",
            "session_id": "test-agent-sess-123"
        });
        let add_res = handle_tool_call(&store, "task_add", &add_args).expect("task_add failed");
        let added: Value = serde_json::from_str(&add_res).unwrap();
        assert_eq!(added["id"], "tk-001");
        assert_eq!(added["priority"], "P0");
        assert_eq!(added["status"], "ready");

        // 2. Claim task via MCP
        let claim_args = json!({
            "id": "1",
            "owner": "claude",
            "session_id": "claude -r 999"
        });
        let claim_res = handle_tool_call(&store, "task_claim", &claim_args).expect("task_claim failed");
        let claimed: Value = serde_json::from_str(&claim_res).unwrap();
        assert_eq!(claimed["status"], "in-progress");
        assert_eq!(claimed["owner"], "claude");

        // 3. Eval task via MCP
        let eval_args = json!({
            "id": "1",
            "actor": "claude"
        });
        let eval_res = handle_tool_call(&store, "task_eval", &eval_args).expect("task_eval failed");
        let eval_val: Value = serde_json::from_str(&eval_res).unwrap();
        assert_eq!(eval_val["passed"], true);

        // 4. Submit review via MCP
        let rev_args = json!({
            "id": "1",
            "actor": "claude",
            "note": "All checks verified"
        });
        let rev_res = handle_tool_call(&store, "task_submit_review", &rev_args).expect("task_submit_review failed");
        let reviewed: Value = serde_json::from_str(&rev_res).unwrap();
        assert_eq!(reviewed["status"], "pending-close");

        // 5. Complete task via MCP
        let comp_args = json!({
            "id": "1",
            "actor": "ken",
            "note": "Signed off"
        });
        let comp_res = handle_tool_call(&store, "task_complete", &comp_args).expect("task_complete failed");
        let completed: Value = serde_json::from_str(&comp_res).unwrap();
        assert_eq!(completed["status"], "done");

        // 6. CPM calculation via MCP
        let cpm_res = handle_tool_call(&store, "task_cpm", &json!({})).expect("task_cpm failed");
        let cpm_val: Value = serde_json::from_str(&cpm_res).unwrap();
        assert_eq!(cpm_val["project_duration_seconds"], 3600);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_mcp_unknown_tool() {
        let (store, tmp) = setup_test_store("unknown");
        let res = handle_tool_call(&store, "non_existent_tool", &json!({}));
        assert!(res.is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
