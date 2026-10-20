use chrono::{Duration, Local};

/// Parser for human-friendly & `todo.txt` shorthand syntax.
/// Supports zsh-safe unquoted priorities (`p0`, `p1`, `!1`, `(A)`), natural dates (`due:today`, `due:tomorrow`, `due:+3d`),
/// `+project`, `@owner`, and `#tag`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTodo {
    pub title: String,
    pub priority: String,
    pub owner: String,
    pub project: Option<String>,
    pub due: Option<String>,
    pub session_id: Option<String>,
    pub blocked_by: Vec<String>,
    pub tags: Vec<String>,
    pub duration: Option<String>,
}

pub fn resolve_natural_date(raw: &str) -> String {
    let today = Local::now().date_naive();
    match raw.trim().to_lowercase().as_str() {
        "today" | "tod" => today.format("%Y-%m-%d").to_string(),
        "tomorrow" | "tom" | "tmrw" => (today + Duration::days(1)).format("%Y-%m-%d").to_string(),
        "next-week" | "1w" | "+7d" => (today + Duration::days(7)).format("%Y-%m-%d").to_string(),
        s if s.starts_with('+') && s.ends_with('d') => {
            if let Ok(days) = s[1..s.len() - 1].parse::<i64>() {
                (today + Duration::days(days))
                    .format("%Y-%m-%d")
                    .to_string()
            } else {
                raw.to_string()
            }
        }
        _ => raw.to_string(),
    }
}

pub fn parse_todotxt(input: &str, default_owner: &str) -> ParsedTodo {
    let mut priority = "P2".to_string();
    let mut owner = default_owner.to_string();
    let mut project: Option<String> = None;
    let mut due: Option<String> = None;
    let mut duration: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut blocked_by: Vec<String> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    let mut words: Vec<&str> = Vec::new();

    for (idx, token) in input.split_whitespace().enumerate() {
        // 1a. Classic todo.txt priority: (A) -> P0, (B) -> P1, (C) -> P2, (D) -> P3
        if idx == 0 && token.len() == 3 && token.starts_with('(') && token.ends_with(')') {
            let letter = token.chars().nth(1).unwrap_or('C').to_ascii_uppercase();
            priority = match letter {
                'A' => "P0".to_string(),
                'B' => "P1".to_string(),
                'C' => "P2".to_string(),
                _ => "P3".to_string(),
            };
            continue;
        }

        // 1b. Zsh-safe unquoted priority tokens: p0, p1, p2, p3, !0, !1, !2, !3
        let lower_tok = token.to_lowercase();
        if matches!(
            lower_tok.as_str(),
            "p0" | "p1" | "p2" | "p3" | "!0" | "!1" | "!2" | "!3"
        ) {
            let digit = lower_tok.chars().nth(1).unwrap_or('2');
            priority = format!("P{}", digit);
            continue;
        }

        // 2. @owner (e.g. @ken, @hermes, @pi, @codex, @claude)
        if let Some(rest) = token.strip_prefix('@')
            && !rest.is_empty()
        {
            owner = rest.to_lowercase();
            continue;
        }

        // 3. +project (e.g. +emba -> [[emba]], +agent-os -> [[agent-os]])
        if let Some(rest) = token.strip_prefix('+')
            && !rest.is_empty()
        {
            project = Some(format!("[[{}]]", rest));
            if !tags.contains(&rest.to_lowercase()) {
                tags.push(rest.to_lowercase());
            }
            continue;
        }

        // 4. #tag (e.g. #ai-tools)
        if let Some(rest) = token.strip_prefix('#')
            && !rest.is_empty()
        {
            tags.push(rest.to_lowercase());
            continue;
        }

        // 5. ~duration estimate (e.g. ~15m, ~2h, ~45s, ~3d)
        if let Some(rest) = token.strip_prefix('~')
            && !rest.is_empty()
            && (rest.ends_with('s')
                || rest.ends_with('m')
                || rest.ends_with('h')
                || rest.ends_with('d')
                || rest.ends_with('w'))
        {
            duration = Some(rest.to_lowercase());
            continue;
        }

        // 6. key:value pairs (due:tomorrow, due:+3d, dur:15m, est:2h, p:P1, owner:pi, sid:8y9380573586)
        if let Some((k, v)) = token.split_once(':') {
            match k.to_lowercase().as_str() {
                "due" if !v.is_empty() => {
                    due = Some(resolve_natural_date(v));
                    continue;
                }
                "dur" | "duration" | "est" if !v.is_empty() => {
                    duration = Some(v.to_lowercase());
                    continue;
                }
                "p" | "pri" | "priority" if !v.is_empty() => {
                    priority = v.to_uppercase();
                    continue;
                }
                "owner" | "agent" if !v.is_empty() => {
                    owner = v.to_lowercase();
                    continue;
                }
                "sid" | "session" | "session_id" if !v.is_empty() => {
                    session_id = Some(v.to_string());
                    continue;
                }
                "after" | "needs" | "dep" | "blocked_by" | "wait_on" if !v.is_empty() => {
                    for id in v.split(',') {
                        let trimmed = id.trim();
                        if !trimmed.is_empty() {
                            let norm = if let Ok(n) = trimmed.parse::<u32>() {
                                format!("tk-{:03}", n)
                            } else if trimmed.starts_with("tk-") {
                                trimmed.to_string()
                            } else {
                                format!("tk-{}", trimmed)
                            };
                            if !blocked_by.contains(&norm) {
                                blocked_by.push(norm);
                            }
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }

        words.push(token);
    }

    if !tags.iter().any(|t| t == "task") {
        tags.insert(0, "task".to_string());
    }

    ParsedTodo {
        title: words.join(" "),
        priority,
        owner,
        project,
        due,
        session_id,
        blocked_by,
        tags,
        duration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_todotxt_shorthand() {
        let parsed = parse_todotxt(
            "(A) Benchmark Pele vs Mermaid +obsidian @pi due:2026-10-20 #cli-tools",
            "ken",
        );
        assert_eq!(parsed.title, "Benchmark Pele vs Mermaid");
        assert_eq!(parsed.priority, "P0");
        assert_eq!(parsed.owner, "pi");
        assert_eq!(parsed.project.as_deref(), Some("[[obsidian]]"));
        assert_eq!(parsed.due.as_deref(), Some("2026-10-20"));
        assert!(parsed.tags.contains(&"cli-tools".to_string()));
    }

    #[test]
    fn test_zsh_safe_unquoted_syntax() {
        let parsed = parse_todotxt("p1 Review EMBA Supply Chain +emba @ken due:today", "ken");
        assert_eq!(parsed.title, "Review EMBA Supply Chain");
        assert_eq!(parsed.priority, "P1");
        assert_eq!(parsed.owner, "ken");
        assert!(parsed.due.is_some());
    }

    #[test]
    fn test_parse_duration_tokens() {
        let parsed = parse_todotxt("p0 Run deep research ~15m @hermes", "ken");
        assert_eq!(parsed.title, "Run deep research");
        assert_eq!(parsed.priority, "P0");
        assert_eq!(parsed.owner, "hermes");
        assert_eq!(parsed.duration.as_deref(), Some("15m"));

        let parsed2 = parse_todotxt("p1 Refactor database schema dur:2h @codex", "ken");
        assert_eq!(parsed2.title, "Refactor database schema");
        assert_eq!(parsed2.duration.as_deref(), Some("2h"));
    }
}
