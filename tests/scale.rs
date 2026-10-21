//! Scale probe: how each agent-facing command behaves on a large store.
//! Opt-in and slow: `cargo test --release --test scale -- --ignored --nocapture`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TK: &str = env!("CARGO_BIN_EXE_tk");

fn run(root: &Path, args: &[&str]) -> Duration {
    let vault = root.join("vault");
    let start = Instant::now();
    let out = Command::new(TK)
        .args(args)
        .current_dir(&vault)
        .env("HOME", root.join("home"))
        .env("USER", "tester")
        .env("TASKMD_DIR", &vault)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let took = start.elapsed();
    assert!(
        out.status.success(),
        "tk {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    took
}

#[test]
#[ignore]
fn scale_commands_on_3000_cards() {
    let n = 3000;
    let root = std::env::temp_dir().join(format!("tkmd-scale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    std::fs::create_dir_all(root.join("vault")).unwrap();
    run(&root, &["add", "p1 template card"]);

    let vault = root.join("vault");
    let template_path = std::fs::read_dir(&vault)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.starts_with("tk-001"))
        })
        .unwrap();
    let template = std::fs::read_to_string(&template_path).unwrap();
    for i in 2..=n {
        let id = format!("tk-{:03}", i);
        std::fs::write(
            vault.join(format!("{}-card-{}.md", id, i)),
            template.replace("id: tk-001", &format!("id: {}", id)),
        )
        .unwrap();
    }

    let last = n.to_string();
    let mut report = Vec::new();
    for (label, args) in [
        ("ls", vec!["ls"]),
        ("show last", vec!["show", last.as_str()]),
        ("add", vec!["add", "p1 one more"]),
        ("claim last", vec!["c", last.as_str(), "-o", "agent"]),
        ("review last", vec!["r", last.as_str(), "-a", "agent"]),
        (
            "dep last needs 1",
            vec!["dep", last.as_str(), "--needs", "1"],
        ),
    ] {
        let t = run(&root, &args);
        report.push((label, t));
    }
    for (label, t) in &report {
        eprintln!("{:>18}: {:>8.1?}  ({} cards)", label, t, n);
    }
    let _ = std::fs::remove_dir_all(&root);
    for (label, t) in report {
        assert!(
            t < Duration::from_secs(3),
            "`{}` took {:?} on {} cards",
            label,
            t,
            n
        );
    }
}
