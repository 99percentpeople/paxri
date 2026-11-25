use gpui::{Keystroke, Modifiers};

pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
    } else {
        text.into_bytes()
    }
}

pub fn sequence(key: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    if mods.platform {
        return None;
    }
    let modifier = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.control);
    let name = key.key.as_str();
    let cursor_key = match name {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(suffix) = cursor_key {
        let prefix = if application_cursor || name.starts_with('f') {
            "\x1bO"
        } else {
            "\x1b["
        };
        return Some(
            if modifier == 1 {
                format!("{prefix}{suffix}")
            } else {
                format!("\x1b[1;{modifier}{suffix}")
            }
            .into_bytes(),
        );
    }
    let tilde = match name {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" | "prior" => Some(5),
        "pagedown" | "next" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(number) = tilde {
        return Some(
            if modifier == 1 {
                format!("\x1b[{number}~")
            } else {
                format!("\x1b[{number};{modifier}~")
            }
            .into_bytes(),
        );
    }
    let value = match name {
        "enter" => "\r",
        "backspace" => "\x7f",
        "tab" if mods.shift => "\x1b[Z",
        "tab" => "\t",
        "escape" => "\x1b",
        "space" if mods.control => return Some(vec![0]),
        k if mods.control
            && k.len() == 1
            && matches!(k.as_bytes()[0].to_ascii_uppercase(), b'@'..=b'_') =>
        {
            return Some(vec![k.as_bytes()[0] & 31]);
        }
        k if mods.alt && !mods.control && !cfg!(target_os = "macos") && k.len() == 1 => {
            return Some(format!("\x1b{}", key.key_char.as_deref().unwrap_or(k)).into_bytes());
        }
        _ => return None,
    };
    Some(value.as_bytes().to_vec())
}

/// Apply virtual modifiers to an IME commit. A latched modifier affects its first
/// key, while a physically held modifier continues across the rest of the commit.
pub fn committed_text(text: &str, first: Modifiers, held: Modifiers) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut mods = first;
    while let Some(mut ch) = chars.next() {
        if ch == '\r' && chars.peek() == Some(&'\n') {
            chars.next();
        }
        ch = match ch {
            '\n' => '\r',
            '\x08' => '\x7f',
            other => other,
        };
        if mods.shift {
            const PLAIN: &str = "`1234567890-=[]\\;',./";
            const SHIFTED: &[u8] = b"~!@#$%^&*()_+{}|:\"<>?";
            ch = if ch.is_ascii() {
                PLAIN
                    .find(ch)
                    .map_or(ch.to_ascii_uppercase(), |i| char::from(SHIFTED[i]))
            } else {
                ch
            };
        }
        if mods.alt {
            bytes.push(0x1b);
        }
        if mods.shift && ch == '\t' {
            bytes.extend_from_slice(b"\x1b[Z");
        } else if mods.control && ch.is_ascii() {
            let byte = ch as u8;
            bytes.push(match byte.to_ascii_uppercase() {
                b'@'..=b'_' => byte & 31,
                b' ' | b'2' => 0,
                b'3'..=b'7' => byte - b'3' + 27,
                b'8' | b'?' => 127,
                _ => byte,
            });
        } else if mods.shift && !ch.is_ascii() {
            for upper in ch.to_uppercase() {
                bytes.extend_from_slice(upper.encode_utf8(&mut [0; 4]).as_bytes());
            }
        } else {
            bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        }
        mods = held;
    }
    bytes
}

/// Named keys on a touch keyboard use the same cursor-key mode as a physical keyboard.
pub fn named_key(name: &str, mods: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    let mut key = Keystroke::parse(name).ok()?;
    key.modifiers = mods;
    let mut bytes = sequence(&key, application_cursor)?;
    if mods.alt && matches!(name, "enter" | "backspace" | "tab" | "escape") {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_preserves_unicode_and_frames_multiline_input() {
        assert_eq!(paste(" 中\r\n文\n ", false), " 中\r文\r ".as_bytes());
        assert_eq!(
            paste("中\n\x1b[201~文", true),
            "\x1b[200~中\r[201~文\x1b[201~".as_bytes()
        );
    }

    #[test]
    fn virtual_modifiers_apply_to_the_first_key_or_the_whole_commit() {
        let ctrl = Modifiers::control();
        let plain = Modifiers::default();
        let both = Modifiers { alt: true, ..ctrl };
        assert_eq!(committed_text("cd", ctrl, plain), b"\x03d");
        assert_eq!(committed_text("cd", both, both), b"\x1b\x03\x1b\x04");
        assert_eq!(
            committed_text("1z你", Modifiers::shift(), Modifiers::shift()),
            "!Z你".as_bytes()
        );
        assert_eq!(committed_text("a\r\nb\x08", plain, plain), b"a\rb\x7f");
        assert_eq!(named_key("left", both, false).unwrap(), b"\x1b[1;7D");
        assert_eq!(
            named_key("backspace", Modifiers::alt(), false).unwrap(),
            b"\x1b\x7f"
        );
    }

    #[test]
    fn special_keys_keep_modifiers_and_application_cursor_mode() {
        for (s, app, bytes) in [
            ("up", true, b"\x1bOA".as_slice()),
            ("up", false, b"\x1b[A"),
            ("ctrl-up", true, b"\x1b[1;5A"),
            ("shift-f2", false, b"\x1b[1;2Q"),
            ("alt-delete", false, b"\x1b[3;3~"),
            ("ctrl-f5", false, b"\x1b[15;5~"),
            ("f1", false, b"\x1bOP"),
            ("ctrl-c", false, b"\x03"),
        ] {
            assert_eq!(
                sequence(&Keystroke::parse(s).unwrap(), app).unwrap(),
                bytes,
                "{s}"
            );
        }
    }
}
