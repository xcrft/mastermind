//! Shared escaping for human-readable terminal output.

pub(crate) fn unsafe_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{0080}'..='\u{009f}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

pub(crate) fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if unsafe_character(character) {
            escaped.extend(character.escape_unicode());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

/// Escape raw terminal controls in an already serialized JSON line without
/// changing its JSON meaning. `serde_json` handles C0 controls inside strings;
/// this additionally makes DEL, C1, and bidi controls visible.
pub(crate) fn escape_json_line(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if unsafe_character(character) {
            escaped.push_str(&format!("\\u{:04x}", character as u32));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_json_escaping_remain_visible_and_parseable() {
        assert_eq!(escape("ok\n\u{202e}"), "ok\\u{a}\\u{202e}");

        let serialized = serde_json::to_string("ok\u{7}\u{202e}").unwrap();
        let safe = escape_json_line(&serialized);
        assert_eq!(safe, "\"ok\\u0007\\u202e\"");
        assert_eq!(
            serde_json::from_str::<String>(&safe).unwrap(),
            "ok\u{7}\u{202e}"
        );
    }
}
