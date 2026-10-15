//! Finding a URL in a row of the grid.
//!
//! A terminal that cannot open a link makes the user select it by hand, and a URL is the one
//! kind of text hand-selection is worst at: it is long, it wraps, and one character short of the
//! end is a link that silently goes somewhere else. Agent CLIs and build tools print sign-in
//! URLs, doc links and error references constantly, and copying them out a character at a time
//! is the sort of friction that is invisible in a feature list and constant in use.
//!
//! **`http` and `https`, and nothing else.** Every other scheme on a Windows machine is a
//! registered launcher for some program — `ms-`, `steam:`, `file:` and whatever else is in the
//! registry — and the text here came out of a pipe. Anything that can print to the terminal
//! could otherwise offer the user a one-click launch of a local handler, which is the same shape
//! as the OSC 52 clipboard-read that `term::spawn` refuses, and it is refused for the same
//! reason. Opening also takes a modifier, so a stray click on a log file cannot do it by
//! accident.
//!
//! The row is expected one `char` per column, the way the grid stores it, so a column index and
//! a character index are the same number. Wide characters occupy their spacer cell, which is
//! never part of a URL.

use std::ops::Range;

const SCHEMES: [&str; 2] = ["https://", "http://"];

/// Characters that end a URL wherever they appear.
///
/// Space is the obvious one; the others are the characters a terminal is likely to have put
/// *around* a link rather than in it — quotes from a JSON payload, angle brackets from the
/// convention for wrapping a bare URL in prose.
fn terminates(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | '\\')
}

/// Trailing characters that are far more likely to be punctuation than part of the link.
///
/// `https://example.com/page.` at the end of a sentence is the common case, and so is a link in
/// brackets. A closing bracket is only dropped when the URL does not contain its opener, because
/// plenty of real URLs — Wikipedia's especially — carry balanced ones.
fn trim_trailing(url: &str) -> &str {
    let mut end = url.len();
    while let Some(c) = url[..end].chars().next_back() {
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => !url[..end].contains('('),
            ']' => !url[..end].contains('['),
            '}' => !url[..end].contains('{'),
            _ => false,
        };
        if !drop {
            break;
        }
        end -= c.len_utf8();
    }
    &url[..end]
}

/// Every URL in one row, as ranges of character positions.
pub fn find(row: &str) -> Vec<Range<usize>> {
    let chars: Vec<char> = row.chars().collect();
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        // A scheme only starts a URL at a boundary, or `not-quite-https://x` would match from
        // the middle of a word that merely ends in one.
        let boundary = i == 0 || terminates(chars[i - 1]) || matches!(chars[i - 1], '(' | '[' | '{');
        let scheme = boundary.then(|| {
            SCHEMES.iter().find(|s| {
                let s: Vec<char> = s.chars().collect();
                chars[i..].len() >= s.len()
                    && chars[i..i + s.len()]
                        .iter()
                        .map(|c| c.to_ascii_lowercase())
                        .eq(s.iter().copied())
            })
        });
        let Some(scheme) = scheme.flatten() else {
            i += 1;
            continue;
        };

        let mut end = i + scheme.chars().count();
        while end < chars.len() && !terminates(chars[end]) {
            end += 1;
        }

        // Nothing after the scheme is not a link, it is the word "https://" on its own.
        let text: String = chars[i..end].iter().collect();
        let trimmed = trim_trailing(&text);
        let length = trimmed.chars().count();
        if length > scheme.chars().count() {
            out.push(i..i + length);
        }
        i = end.max(i + 1);
    }

    out
}

/// The URL covering `column`, if the pointer is over one.
pub fn at(row: &str, column: usize) -> Option<String> {
    let chars: Vec<char> = row.chars().collect();
    let range = find(row).into_iter().find(|r| r.contains(&column))?;
    Some(chars[range].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_url_is_found_anywhere_in_the_line() {
        let row = "see https://example.com/a for more";
        assert_eq!(at(row, 4).as_deref(), Some("https://example.com/a"));
        assert_eq!(at(row, 24).as_deref(), Some("https://example.com/a"));
        // One past the end is the space, and the word after it is not a link.
        assert_eq!(at(row, 25), None);
        assert_eq!(at(row, 0), None);
    }

    #[test]
    fn sentence_punctuation_is_not_part_of_the_link() {
        assert_eq!(at("go to https://example.com/page.", 10).as_deref(), Some("https://example.com/page"));
        assert_eq!(at("(https://example.com/a)", 5).as_deref(), Some("https://example.com/a"));
        assert_eq!(at("see https://example.com/a, then", 8).as_deref(), Some("https://example.com/a"));
    }

    /// Wikipedia's are the usual example, and truncating one silently goes somewhere else.
    #[test]
    fn brackets_that_belong_to_the_url_are_kept() {
        let row = "https://en.wikipedia.org/wiki/Bracket_(disambiguation)";
        assert_eq!(at(row, 0).as_deref(), Some(row));
    }

    #[test]
    fn only_http_and_https_are_offered() {
        for row in [
            "file:///C:/Windows/System32/cmd.exe",
            "steam://run/440",
            "ms-settings:windowsupdate",
            "ftp://example.com/x",
            "javascript:alert(1)",
        ] {
            assert!(find(row).is_empty(), "{row} was offered as a link");
        }
    }

    #[test]
    fn a_scheme_in_the_middle_of_a_word_is_not_a_link() {
        assert!(find("nothttps://example.com").is_empty());
        // ...and a scheme with nothing after it is a word, not a link.
        assert!(find("https:// and then").is_empty());
    }

    #[test]
    fn quotes_and_angle_brackets_around_a_url_are_not_in_it() {
        assert_eq!(at("\"https://example.com/a\"", 5).as_deref(), Some("https://example.com/a"));
        assert_eq!(at("<https://example.com/a>", 5).as_deref(), Some("https://example.com/a"));
    }

    #[test]
    fn two_links_on_one_row_are_told_apart() {
        let row = "https://a.example one https://b.example";
        assert_eq!(find(row).len(), 2);
        assert_eq!(at(row, 0).as_deref(), Some("https://a.example"));
        assert_eq!(at(row, 25).as_deref(), Some("https://b.example"));
    }

    #[test]
    fn the_scheme_is_matched_without_regard_to_case() {
        assert_eq!(at("HTTPS://Example.COM/a", 0).as_deref(), Some("HTTPS://Example.COM/a"));
    }
}
