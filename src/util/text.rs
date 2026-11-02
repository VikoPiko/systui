//! Terminal-injection-safe text handling.

/// Replace control characters and bidi/format overrides so untrusted strings
/// (process names, paths, command lines) can never emit terminal escapes.
/// Tabs and newlines become spaces; other controls become `?`.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => ' ',
            c if c.is_control() => '?',
            // bidi embeddings/overrides/isolates, zero-width and BOM
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}' => '?',
            c => c,
        })
        .collect()
}

/// Remove ANSI/VT escape sequences (colors, cursor moves, OSC titles) from text that
/// came from another program, e.g. container logs. Run `sanitize` afterwards for
/// anything that survives. Unterminated sequences are dropped to the end of the line.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            // CSI: ESC [ params... final byte 0x40-0x7E
            Some('[') => {
                it.next();
                for n in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL | ESC \
            Some(']') => {
                it.next();
                while let Some(n) = it.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        if it.peek() == Some(&'\\') {
                            it.next();
                        }
                        break;
                    }
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            // two-character escapes (ESC c, ESC 7 ...)
            Some(_) => {
                it.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_color_cursor_and_osc_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m plain"), "red plain");
        assert_eq!(strip_ansi("a\u{1b}[2J\u{1b}[1;1Hb"), "ab");
        assert_eq!(strip_ansi("x\u{1b}]0;evil title\u{7}y"), "xy");
        assert_eq!(
            strip_ansi("x\u{1b}]8;;http://e\u{1b}\\link\u{1b}]8;;\u{1b}\\"),
            "xlink"
        );
        assert_eq!(strip_ansi("\u{1b}creset"), "reset");
        assert_eq!(strip_ansi("trailing \u{1b}"), "trailing ");
        assert_eq!(
            strip_ansi("bad \u{1b}[31"),
            "bad ",
            "unterminated CSI is dropped"
        );
        assert_eq!(strip_ansi("no escapes é日本"), "no escapes é日本");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_then_sanitize_leaves_nothing_dangerous() {
        let nasty = "\u{1b}[31m\u{1b}]0;t\u{7}ok\u{8}\u{1b}[";
        let clean = sanitize(&strip_ansi(nasty));
        assert!(!clean.chars().any(|c| c.is_control()), "{clean:?}");
    }

    #[test]
    fn strips_ansi_and_controls() {
        let s = sanitize("evil\x1b[2J\x1b]0;title\x07name\r\nx\ty");
        assert!(!s.contains('\x1b') && !s.contains('\x07') && !s.contains('\n'));
        assert_eq!(s, "evil?[2J?]0;title?name  x y");
    }

    #[test]
    fn neutralizes_bidi_overrides() {
        assert_eq!(sanitize("a\u{202E}b"), "a?b");
    }

    #[test]
    fn keeps_normal_unicode() {
        assert_eq!(sanitize("héllo 日本語"), "héllo 日本語");
    }
}
