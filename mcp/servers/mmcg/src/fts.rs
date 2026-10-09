//! Preserve explicit FTS syntax while admitting plain hyphenated search terms.

use std::borrow::Cow;

pub(crate) fn query(input: &str) -> Cow<'_, str> {
    if !input.contains('-') {
        return Cow::Borrowed(input);
    }
    let mut output = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    let mut quoted = false;
    while let Some((start, ch)) = chars.next() {
        if ch == '"' {
            quoted = !quoted;
            output.push(ch);
        } else if !quoted && bareword(ch) {
            let mut end = start + ch.len_utf8();
            while let Some(&(position, next)) = chars.peek() {
                if !bareword(next) && next != '-' {
                    break;
                }
                chars.next();
                end = position + next.len_utf8();
            }
            let term = &input[start..end];
            if term.contains('-') {
                output.push('"');
                output.push_str(term);
                output.push('"');
            } else {
                output.push_str(term);
            }
        } else {
            output.push(ch);
        }
    }
    Cow::Owned(output)
}

fn bareword(ch: char) -> bool {
    !ch.is_ascii() || ch.is_ascii_alphanumeric() || matches!(ch, '_' | '\u{001a}')
}
