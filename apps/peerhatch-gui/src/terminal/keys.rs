use gpui::Keystroke;

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

#[cfg(test)]
mod tests {
    use super::*;

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
