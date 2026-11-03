//! Small terminal output helpers.

use std::fmt::Display;
use std::io::Write;

use console::{measure_text_width, pad_str, truncate_str, Alignment};

/// Prints a line to stdout; exits quietly when the reader went away
/// (`dc search … | head`) instead of panicking like `println!`.
pub fn line(text: impl Display) {
    let mut out = std::io::stdout().lock();
    if let Err(e) = writeln!(out, "{text}") {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
    }
}

/// Shortens `text` (may contain ANSI styling) to at most `width` display
/// columns, ending in `…` only when something was cut.
/// (`console::truncate_str` also cuts text that would fit exactly.)
pub fn fit(text: &str, width: usize) -> String {
    if measure_text_width(text) <= width {
        text.to_owned()
    } else {
        truncate_str(text, width, "…").into_owned()
    }
}

/// [`fit`] and pad to exactly `width` columns.
pub fn cell(text: &str, width: usize, align: Alignment) -> String {
    pad_str(&fit(text, width), width, align, None).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting() {
        assert_eq!(fit("2024-06-29", 10), "2024-06-29");
        assert_eq!(fit("536.9万", 7), "536.9万");
        assert_eq!(fit("2024-06-29", 9), "2024-06-…");
        assert_eq!(
            measure_text_width(&cell("一样的月光测试", 6, Alignment::Left)),
            6
        );
        assert_eq!(cell("ab", 4, Alignment::Right), "  ab");
    }
}
