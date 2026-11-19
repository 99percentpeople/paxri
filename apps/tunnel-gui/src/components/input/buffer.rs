use std::ops::Range;

pub(super) struct TextBuffer<const MULTILINE: bool> {
    pub text: String,
    pub read_only: bool,
    pub anchor: usize,
    pub cursor: usize,
    pub marked: Option<Range<usize>>,
}

pub(super) fn byte_at_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units + c.len_utf16() > offset {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn normalize(value: &str, multiline: bool) -> String {
    if multiline {
        value.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        value.replace(['\r', '\n'], "")
    }
}

impl<const MULTILINE: bool> TextBuffer<MULTILINE> {
    pub fn new(text: &str) -> Self {
        let text = normalize(text, MULTILINE);
        Self {
            read_only: false,
            anchor: text.len(),
            cursor: text.len(),
            text,
            marked: None,
        }
    }

    pub fn read_only(text: &str) -> Self {
        Self {
            read_only: true,
            anchor: 0,
            cursor: 0,
            ..Self::new(text)
        }
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn utf16(&self, byte: usize) -> usize {
        self.text[..byte].encode_utf16().count()
    }

    pub fn replace(&mut self, range: Option<Range<usize>>, value: &str) -> Range<usize> {
        if self.read_only {
            return self.cursor..self.cursor;
        }
        let range = range
            .map(|r| byte_at_utf16(&self.text, r.start)..byte_at_utf16(&self.text, r.end))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let value = normalize(value, MULTILINE);
        self.text.replace_range(range.clone(), &value);
        self.cursor = range.start + value.len();
        self.anchor = self.cursor;
        self.marked = None;
        range.start..self.cursor
    }
    /// IMEs report their selection in UTF-16 units of the original composition.
    pub fn mark(&mut self, added: Range<usize>, original: &str, selection: Option<Range<usize>>) {
        if added.is_empty() {
            return;
        }
        if let Some(selection) = selection {
            let offset = |units| {
                let prefix = &original[..byte_at_utf16(original, units)];
                added.start + normalize(prefix, MULTILINE).len()
            };
            self.anchor = offset(selection.start);
            self.cursor = offset(selection.end);
        }
        self.marked = Some(added);
    }
}

#[cfg(test)]
mod form_controls {
    use super::*;

    #[test]
    fn single_line_input_removes_pasted_and_initial_line_breaks() {
        let mut input = TextBuffer::<false>::new("a\r\nb");
        input.replace(None, "\rc\nd");
        assert_eq!(input.text, "abcd");
        assert_eq!(input.selection(), 4..4);
    }

    #[test]
    fn read_only_input_keeps_text_and_selection_during_edits_and_composition() {
        let mut buffer = TextBuffer::<false>::read_only("p2p1.α\nβ");
        assert_eq!(buffer.selection(), 0..0);
        buffer.anchor = "p2p1.α".len();
        buffer.cursor = "p2p1.".len();
        let selection = buffer.selection();
        buffer.replace(None, "");
        buffer.replace(None, "pasted");
        let added = buffer.replace(Some(0..4), "漢");
        buffer.mark(added, "漢", Some(1..1));
        assert_eq!(buffer.text, "p2p1.αβ");
        assert_eq!(buffer.selection(), selection);
        assert_eq!(&buffer.text[selection], "α");
        assert!(buffer.marked.is_none());
    }

    #[test]
    fn textarea_preserves_empty_lines_and_replaces_a_reversed_selection() {
        let mut input = TextBuffer::<true>::new("α\r\n\r🐈");
        assert_eq!(input.text, "α\n\n🐈");
        input.cursor = "α".len();
        input.replace(None, "\r\n新\r");
        assert_eq!(input.text, "α\n新\n");
        assert_eq!(input.selection(), input.text.len()..input.text.len());
    }

    #[test]
    fn ime_replacement_uses_utf16_offsets_and_the_marked_range() {
        let mut input = TextBuffer::<true>::new("x🐈\nabc");
        let marked = input.replace(Some(1..3), "中文");
        assert_eq!(input.utf16(marked.end), 3);
        input.mark(marked, "中文", Some(1..2));
        assert_eq!(&input.text[input.selection()], "文");
        input.replace(None, "字");
        assert_eq!(input.text, "x字\nabc");
        assert!(input.marked.is_none());
    }
    #[test]
    fn textarea_ime_selection_uses_the_normalized_newlines() {
        let mut input = TextBuffer::<true>::new("");
        let composition = "🐈\r\n字";
        let added = input.replace(None, composition);
        input.mark(added, composition, Some(2..4));
        assert_eq!(&input.text[input.selection()], "\n");
    }
}
