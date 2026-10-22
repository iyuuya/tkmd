//! Terminal column arithmetic. CJK and other wide glyphs occupy two cells,
//! so `chars().count()` and `{:<N}` misalign tables that contain them.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Number of terminal cells `s` occupies.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate `s` to at most `max_cols` cells, ending in `…` if anything was cut.
pub fn truncate(s: &str, max_cols: usize) -> String {
    if width(s) <= max_cols {
        return s.to_string();
    }
    if max_cols == 0 {
        return String::new();
    }
    let budget = max_cols - 1;
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Left-align `s` in a field of `cols` cells (no truncation).
pub fn pad(s: &str, cols: usize) -> String {
    format!("{s}{}", " ".repeat(cols.saturating_sub(width(s))))
}

/// Truncate then left-align, so the result is exactly `cols` cells wide.
pub fn fit(s: &str, cols: usize) -> String {
    pad(&truncate(s, cols), cols)
}

/// Remove SGR escape sequences so tests can measure what the terminal shows.
#[cfg(test)]
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            chars.by_ref().find(|&c| c == 'm');
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_width_ascii_is_one_cell_per_char() {
        assert_eq!(width("abc"), 3);
    }

    #[test]
    fn test_width_cjk_is_two_cells_per_char() {
        assert_eq!(width("日本語abc"), 9);
    }

    #[test]
    fn test_truncate_returns_input_when_it_fits() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("日本", 4), "日本");
    }

    #[test]
    fn test_truncate_appends_ellipsis_when_cut() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("日本abc語", 6), "日本a…");
    }

    #[test]
    fn test_truncate_drops_wide_char_that_would_straddle_limit() {
        assert_eq!(truncate("日本語", 5), "日本…");
        assert_eq!(width(&truncate("あいうえお", 8)), 7);
    }

    #[test]
    fn test_truncate_zero_width_is_empty() {
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn test_pad_fills_by_cells_not_chars() {
        assert_eq!(pad("日本", 6), "日本  ");
        assert_eq!(pad("toolong", 3), "toolong");
    }

    #[test]
    fn test_fit_always_yields_exact_width() {
        for s in ["", "abc", "日本", "日本語のテスト文字列"] {
            assert_eq!(width(&fit(s, 9)), 9, "fit({s:?}, 9) should be 9 cells wide");
        }
    }
}
