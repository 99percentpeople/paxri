use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

const GAP: usize = 2;

pub(super) struct Report {
    pub text: String,
    columns: usize,
}

impl Report {
    pub fn new() -> Self {
        let columns = crate::shell::terminal_columns()
            .filter(|&n| n > 1)
            .or_else(|| {
                std::env::var("COLUMNS")
                    .ok()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
            })
            .unwrap_or(100);
        Self {
            text: String::new(),
            columns,
        }
    }

    pub fn line(&mut self, text: &str) {
        self.text.push_str(&printable(text));
        self.text.push('\n');
    }

    pub fn heading(&mut self, title: &str) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.line(title);
    }

    pub fn fields(&mut self, fields: &[(&str, &str)]) {
        let label_width = fields
            .iter()
            .map(|(label, _)| label.width())
            .max()
            .unwrap_or(0);
        for &(label, value) in fields {
            let value = printable(value);
            self.text.push_str(label);
            if label_width + GAP + value.width() <= self.columns {
                self.text
                    .extend(std::iter::repeat_n(' ', label_width - label.width() + GAP));
            } else {
                self.text.push('\n');
            }
            self.text.push_str(&value);
            self.text.push('\n');
        }
    }

    pub fn table<const N: usize>(
        &mut self,
        title: &str,
        headings: [&str; N],
        mut rows: Vec<[String; N]>,
    ) {
        self.heading(&format!("{title} ({})", rows.len()));
        if rows.is_empty() {
            return;
        }
        let mut widths = headings.map(UnicodeWidthStr::width);
        for row in &mut rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                // A name or listener error containing a control character stays in its cell.
                if let Cow::Owned(escaped) = printable(cell) {
                    *cell = escaped;
                }
                *width = (*width).max(cell.width());
            }
        }
        if widths.iter().sum::<usize>() + GAP * N.saturating_sub(1) <= self.columns {
            write_row(&mut self.text, headings, &widths);
            for row in &rows {
                write_row(&mut self.text, row.each_ref().map(String::as_str), &widths);
            }
        } else {
            // Keep complete addresses, errors and copyable device IDs on narrow terminals.
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    self.text.push('\n');
                }
                let fields: [_; N] = std::array::from_fn(|i| (headings[i], row[i].as_str()));
                self.fields(&fields);
            }
        }
    }
}

fn write_row<const N: usize>(out: &mut String, row: [&str; N], widths: &[usize; N]) {
    for (i, value) in row.into_iter().enumerate() {
        out.push_str(value);
        if i + 1 < N {
            out.extend(std::iter::repeat_n(' ', widths[i] - value.width() + GAP));
        }
    }
    out.push('\n');
}

fn printable(text: &str) -> Cow<'_, str> {
    if !text.contains(char::is_control) {
        return text.into();
    }
    let mut escaped = String::new();
    for ch in text.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_wide_and_combining_characters_and_keeps_control_text_in_its_row() {
        let mut report = Report {
            text: String::new(),
            columns: 80,
        };
        report.table(
            "Devices",
            ["NAME", "STATE"],
            vec![
                ["树莓派".into(), "Connected".into()],
                ["cafe\u{301}".into(), "Idle\nretry\t1".into()],
            ],
        );
        let mut lines = report.text.lines().skip(1);
        for (line, state) in lines
            .by_ref()
            .zip(["STATE", "Connected", "Idle\\nretry\\t1"])
        {
            let (left, _) = line.split_once(state).unwrap();
            assert_eq!(left.width(), 8);
        }
        assert_eq!(report.text.lines().count(), 4);
    }

    #[test]
    fn narrow_reports_keep_pairing_ids_whole_and_separate_records() {
        let mut report = Report {
            text: String::new(),
            columns: 22,
        };
        let id = "0123456789abcdef".repeat(4);
        report.table(
            "Requests",
            ["NAME", "DEVICE ID"],
            vec![
                ["first".into(), id.clone()],
                ["second".into(), "short".into()],
            ],
        );
        assert_eq!(
            report.text,
            format!(
                "Requests (2)\nNAME       first\nDEVICE ID\n{id}\n\nNAME       second\nDEVICE ID  short\n"
            )
        );
    }
}
