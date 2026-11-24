//! Restore input modes even when their escape sequences preceded the retained PTY output.
use paxri_terminal::managed::HISTORY_LIMIT;
use std::collections::VecDeque;
use vte::{Params, Parser, Perform};

const CHECKPOINT_INTERVAL: u64 = 4096;

#[derive(Clone, Copy, Default)]
struct Modes {
    mouse: Option<u16>,
    encoding: Option<u16>,
    cursor_keys: bool,
    keypad: bool,
    focus: bool,
    alternate: bool,
    disable_alternate_scroll: bool,
    paste: bool,
}

impl Modes {
    fn prefix(self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for mode in [
            self.alternate.then_some(1049),
            self.cursor_keys.then_some(1),
            self.mouse,
            self.encoding,
            self.focus.then_some(1004),
            self.paste.then_some(2004),
        ]
        .into_iter()
        .flatten()
        {
            bytes.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
        }
        if self.disable_alternate_scroll {
            bytes.extend_from_slice(b"\x1b[?1007l");
        }
        if self.keypad {
            bytes.extend_from_slice(b"\x1b=");
        }
        bytes
    }

    fn set(&mut self, mode: u16, enabled: bool) {
        let group = match mode {
            1000 | 1002 | 1003 => Some(&mut self.mouse),
            1005 | 1006 => Some(&mut self.encoding),
            _ => None,
        };
        if let Some(group) = group {
            if enabled {
                *group = Some(mode);
            } else if *group == Some(mode) {
                *group = None;
            }
            return;
        }
        match mode {
            1 => self.cursor_keys = enabled,
            1004 => self.focus = enabled,
            1007 => self.disable_alternate_scroll = !enabled,
            1049 => self.alternate = enabled,
            2004 => self.paste = enabled,
            _ => {}
        }
    }
}

#[derive(Default)]
struct Tracker {
    modes: Modes,
    boundary: bool,
}

impl Perform for Tracker {
    fn print(&mut self, _: char) {
        self.boundary = true;
    }

    fn osc_dispatch(&mut self, _: &[&[u8]], bell_terminated: bool) {
        self.boundary = bell_terminated;
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.boundary = true;
        if ignore || intermediates != b"?" || !matches!(action, 'h' | 'l') {
            return;
        }
        for param in params {
            if let [mode] = param {
                self.modes.set(*mode, action == 'h');
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.boundary = true;
        if !ignore && intermediates.is_empty() {
            match byte {
                b'=' => self.modes.keypad = true,
                b'>' => self.modes.keypad = false,
                b'c' => self.modes = Modes::default(),
                _ => {}
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Replay {
    bytes: VecDeque<u8>,
    written: u64,
    next_checkpoint: u64,
    checkpoints: VecDeque<(u64, Modes)>,
    parser: Parser,
    tracker: Tracker,
}

impl Replay {
    pub(super) fn append(&mut self, data: &[u8]) {
        let tail = &data[data.len().saturating_sub(HISTORY_LIMIT)..];
        let excess = (self.bytes.len() + tail.len()).saturating_sub(HISTORY_LIMIT);
        self.bytes.drain(..excess);
        self.bytes.extend(tail);
        let start = (self.written + data.len() as u64).saturating_sub(HISTORY_LIMIT as u64);
        self.checkpoints.retain(|(pos, _)| *pos >= start);
        let mut remaining = data;
        while !remaining.is_empty() {
            let bulk = self.next_checkpoint.saturating_sub(self.written) as usize;
            if bulk != 0 {
                let n = bulk.min(remaining.len());
                self.parser.advance(&mut self.tracker, &remaining[..n]);
                remaining = &remaining[n..];
                self.written += n as u64;
                continue;
            }
            // A fresh parser may start at ESC, or after a complete character or sequence.
            let at_escape = remaining[0] == 0x1b;
            let pos = self.written;
            let modes = self.tracker.modes;
            self.tracker.boundary = false;
            self.parser.advance(&mut self.tracker, &remaining[..1]);
            self.written += 1;
            remaining = &remaining[1..];
            if at_escape || self.tracker.boundary {
                let checkpoint = if at_escape {
                    (pos, modes)
                } else {
                    (self.written, self.tracker.modes)
                };
                if checkpoint.0 >= start {
                    self.checkpoints.push_back(checkpoint);
                }
                self.next_checkpoint = self.written + CHECKPOINT_INTERVAL;
            }
        }
    }

    pub(super) fn snapshot(&self) -> Vec<u8> {
        let start = self.written - self.bytes.len() as u64;
        if start != 0 {
            for &(pos, modes) in &self.checkpoints {
                let offset = (pos - start) as usize;
                let mut restored = modes.prefix();
                if restored.len() + self.bytes.len() - offset <= HISTORY_LIMIT {
                    restored.extend(self.bytes.range(offset..));
                    return restored;
                }
            }
            // An uninterrupted control string can occupy the entire retained
            // window. Keep the input modes even without a usable checkpoint.
            let mut restored = self.tracker.modes.prefix();
            let offset = (self.bytes.len() + restored.len()).saturating_sub(HISTORY_LIMIT);
            restored.extend(self.bytes.range(offset..));
            return restored;
        }
        self.bytes.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_replay_restores_modes_before_text_and_obeys_later_mode_changes() {
        let mut history = Replay::default();
        history.append(b"\x1b[?1049;1002");
        history.append(b";1006;1;1004;2004h\x1b=\x1b[?1007l");
        let plain = vec![b'a'; HISTORY_LIMIT + 599];
        history.append(&plain);
        history.append(b"\x1b[?1003h\x1b[?1002l\x1b[?1006l\x1b[?1005h");
        let replay = history.snapshot();
        assert!(replay.len() <= HISTORY_LIMIT);
        assert!(replay.starts_with(b"\x1b[?1049h"));
        let mut result = Tracker::default();
        Parser::new().advance(&mut result, &replay);
        let modes = result.modes;
        assert_eq!(modes.mouse, Some(1003));
        assert_eq!(modes.encoding, Some(1005));
        assert!(
            modes.cursor_keys
                && modes.keypad
                && modes.focus
                && modes.alternate
                && modes.paste
                && modes.disable_alternate_scroll
        );
    }
}
