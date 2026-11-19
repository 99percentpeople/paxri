use gpui::Keystroke;

pub fn sequence(key: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    if mods.platform {
        return None;
    }
    let prefix = if application_cursor { "\x1bO" } else { "\x1b[" };
    let value = match key.key.as_str() {
        "up" | "down" | "right" | "left" | "home" | "end" => {
            let suffix = match key.key.as_str() {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            let modifier =
                1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.control);
            return Some(
                if modifier == 1 {
                    format!("{prefix}{suffix}")
                } else {
                    format!("\x1b[1;{modifier}{suffix}")
                }
                .into_bytes(),
            );
        }
        "enter" => "\r",
        "backspace" => "\x7f",
        "tab" if mods.shift => "\x1b[Z",
        "tab" => "\t",
        "escape" => "\x1b",
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
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
