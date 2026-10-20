//! Terminal colour that switches itself off when output is not a terminal.
//! Agents read piped output; escape codes there are pure token noise.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable colour only for a terminal, and never when `NO_COLOR` is set.
pub fn init_for_stdout() {
    use std::io::IsTerminal;
    let on = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    ENABLED.store(on, Ordering::Relaxed);
}

/// Force colour on or off (e.g. an explicit `--color` flag).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// An ANSI escape that renders as nothing when colour is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ansi(pub &'static str);

impl fmt::Display for Ansi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if ENABLED.load(Ordering::Relaxed) {
            f.write_str(self.0)
        } else {
            Ok(())
        }
    }
}
