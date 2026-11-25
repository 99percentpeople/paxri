use super::emulator::Emulator;
use alacritty_terminal::{
    Grid,
    grid::Dimensions,
    index::{Column, Line, Point as GridPoint},
    term::cell::{Cell, Flags, Hyperlink},
};
use url::Url;

const MAX_WRAP_LINES: usize = 16;
const PREFIXES: [&str; 4] = ["https://", "http://", "mailto:", "www."];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLink {
    pub start: GridPoint,
    pub end: GridPoint,
    pub uri: String,
    pub explicit: bool,
}

impl TerminalLink {
    pub fn contains(&self, point: GridPoint) -> bool {
        point >= self.start && point <= self.end
    }
}

/// Only allow external schemes that cannot execute local commands or read local files.
pub fn sanitize_uri(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    let parsed = Url::parse(trimmed).ok()?;
    match parsed.scheme() {
        "http" | "https" => {
            let host = parsed.host_str()?;
            if host.is_empty() {
                return None;
            }
            Some(trimmed.to_owned())
        }
        "mailto" => {
            let (user, domain) = parsed.path().split_once('@')?;
            if user.is_empty() || domain.is_empty() || !domain.contains('.') {
                return None;
            }
            Some(trimmed.to_owned())
        }
        _ => None,
    }
}

/// Insert zero-width break opportunities so long unbroken URLs wrap cleanly in narrow UI cards.
pub fn wrappable_uri(uri: &str) -> String {
    let mut out = String::with_capacity(uri.len() + uri.len() / 8);
    let mut run = 0usize;
    for ch in uri.chars() {
        out.push(ch);
        if matches!(ch, '/' | '?' | '&' | '=' | '#' | '-' | '_' | '.') {
            out.push('\u{200b}');
            run = 0;
        } else {
            run += 1;
            if run >= 24 {
                out.push('\u{200b}');
                run = 0;
            }
        }
    }
    out
}

fn cell_hyperlink(grid: &Grid<Cell>, point: GridPoint) -> Option<Hyperlink> {
    let flags = grid[point].flags;
    if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
        return None;
    }
    if flags.contains(Flags::WIDE_CHAR_SPACER) && point.column.0 > 0 {
        return grid[GridPoint::new(point.line, point.column - 1)].hyperlink();
    }
    grid[point].hyperlink()
}

fn prev_cell(grid: &Grid<Cell>, point: GridPoint, min_line: Line) -> Option<GridPoint> {
    let last_col = grid.last_column();
    if point.column.0 > 0 {
        let prev = GridPoint::new(point.line, point.column - 1);
        if grid[prev].flags.contains(Flags::WIDE_CHAR_SPACER) && prev.column.0 > 0 {
            return Some(GridPoint::new(prev.line, prev.column - 1));
        }
        return Some(prev);
    }
    if point.line <= min_line {
        return None;
    }
    let mut prev = GridPoint::new(point.line - 1i32, last_col);
    if grid[prev].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) && prev.column.0 > 0 {
        prev.column -= 1;
    }
    if grid[prev].flags.contains(Flags::WIDE_CHAR_SPACER) && prev.column.0 > 0 {
        prev.column -= 1;
    }
    Some(prev)
}

fn next_cell(grid: &Grid<Cell>, point: GridPoint, max_line: Line) -> Option<GridPoint> {
    let last_col = grid.last_column();
    let step = if grid[point].flags.contains(Flags::WIDE_CHAR) && point.column < last_col {
        2
    } else {
        1
    };
    let next_col = point.column.0 + step;
    if next_col <= last_col.0 {
        let next = GridPoint::new(point.line, Column(next_col));
        if !grid[next].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            return Some(next);
        }
    }
    if point.line >= max_line {
        return None;
    }
    Some(GridPoint::new(point.line + 1i32, Column(0)))
}

fn osc8_link_at(grid: &Grid<Cell>, point: GridPoint) -> Option<TerminalLink> {
    let target = cell_hyperlink(grid, point)?;
    let uri = sanitize_uri(target.uri())?;
    let min_line = (point.line - MAX_WRAP_LINES as i32).max(grid.topmost_line());
    let max_line = (point.line + MAX_WRAP_LINES as i32).min(grid.bottommost_line());
    let last_col = grid.last_column();

    let mut start = point;
    while let Some(prev) = prev_cell(grid, start, min_line) {
        if cell_hyperlink(grid, prev).as_ref() != Some(&target) {
            break;
        }
        start = prev;
    }
    let mut end = point;
    while let Some(next) = next_cell(grid, end, max_line) {
        if cell_hyperlink(grid, next).as_ref() != Some(&target) {
            break;
        }
        end = next;
    }
    if grid[start].flags.contains(Flags::WIDE_CHAR_SPACER) && start.column.0 > 0 {
        start.column -= 1;
    }
    if grid[end].flags.contains(Flags::WIDE_CHAR) && end.column < last_col {
        end.column += 1;
    }
    Some(TerminalLink {
        start,
        end,
        uri,
        explicit: true,
    })
}

fn is_url_stopper(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_control()
        || matches!(
            ch,
            '<' | '>'
                | '"'
                | '`'
                | '\u{2500}'..='\u{259f}'
                | '\u{e0a0}'..='\u{e0b3}'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '「'
                | '」'
                | '『'
                | '』'
                | '，'
                | '。'
                | '；'
                | '：'
                | '！'
                | '？'
                | '、'
                | '“'
                | '”'
                | '‘'
                | '’'
        )
}

fn trim_url_candidate(mut candidate: &str) -> &str {
    loop {
        let before = candidate.len();
        candidate = candidate.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            while candidate.ends_with(close) {
                let opens = candidate.chars().filter(|&c| c == open).count();
                let closes = candidate.chars().filter(|&c| c == close).count();
                if closes > opens {
                    candidate = &candidate[..candidate.len() - close.len_utf8()];
                } else {
                    break;
                }
            }
        }
        if candidate.len() == before {
            break;
        }
    }
    candidate
}

fn has_valid_prefix_boundary(text: &str, start: usize, prefix: &str) -> bool {
    let Some(prev) = text[..start].chars().next_back() else {
        return true;
    };
    if prefix == "www." {
        !prev.is_ascii_alphanumeric() && !matches!(prev, '.' | '-' | '_' | '/' | '@')
    } else {
        !prev.is_ascii_alphanumeric()
    }
}

fn find_url_at_byte(text: &str, target_byte: usize) -> Option<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        let matched_prefix = PREFIXES.iter().copied().find(|prefix| {
            rest.len() > prefix.len()
                && rest.is_char_boundary(prefix.len())
                && rest[..prefix.len()].eq_ignore_ascii_case(prefix)
                && has_valid_prefix_boundary(text, i, prefix)
        });
        if let Some(prefix) = matched_prefix {
            let start_body = i + prefix.len();
            let mut end = start_body;
            for (offset, ch) in text[start_body..].char_indices() {
                if is_url_stopper(ch) {
                    end = start_body + offset;
                    break;
                }
                end = start_body + offset + ch.len_utf8();
            }
            let trimmed = trim_url_candidate(&text[i..end]);
            let match_end = i + trimmed.len();
            if target_byte >= i && target_byte < match_end {
                let normalized = if prefix == "www." {
                    let after_www = &trimmed[prefix.len()..];
                    let host = after_www.split(['/', '?', '#', ':']).next().unwrap_or("");
                    if !host.contains('.') || host.starts_with('.') || host.ends_with('.') || host.contains("..") {
                        i = match_end.max(i + 1);
                        continue;
                    }
                    format!("https://{trimmed}")
                } else {
                    trimmed.to_owned()
                };
                if let Some(uri) = sanitize_uri(&normalized) {
                    return Some((i, match_end, uri));
                }
            }
            i = match_end.max(i + 1);
            continue;
        }
        let ch = rest.chars().next()?;
        i += ch.len_utf8();
    }
    None
}

impl Emulator {
    /// Detect an explicit OSC 8 hyperlink or plain-text URL at viewport `(row, col_x)`.
    pub fn link_at(&self, row: usize, col_x: f32) -> Option<TerminalLink> {
        let grid = self.term.grid();
        if row >= self.term.screen_lines()
            || col_x < 0.
            || col_x >= grid.columns() as f32
        {
            return None;
        }
        let col = col_x.floor() as usize;
        let line = self.view_line(row);
        let mut point = GridPoint::new(line, Column(col));
        let flags = grid[point].flags;
        if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            return None;
        }
        if flags.contains(Flags::WIDE_CHAR_SPACER) && point.column.0 > 0 {
            point.column -= 1;
        }

        if let Some(link) = osc8_link_at(grid, point) {
            return Some(link);
        }

        let last_col = grid.last_column();
        let mut start_line = line;
        for _ in 0..MAX_WRAP_LINES {
            if start_line <= grid.topmost_line()
                || !grid[start_line - 1i32][last_col]
                    .flags
                    .contains(Flags::WRAPLINE)
            {
                break;
            }
            start_line -= 1;
        }
        let mut end_line = line;
        for _ in 0..MAX_WRAP_LINES {
            if end_line >= grid.bottommost_line()
                || !grid[end_line][last_col].flags.contains(Flags::WRAPLINE)
            {
                break;
            }
            end_line += 1;
        }

        let cols = grid.columns();
        let mut text = String::new();
        let mut cells: Vec<(usize, usize, GridPoint, bool)> = Vec::new();
        let mut target_byte = None;
        for r in start_line.0..=end_line.0 {
            for c in 0..cols {
                let p = GridPoint::new(Line(r), Column(c));
                let cell = &grid[p];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let byte_start = text.len();
                if cell.flags.contains(Flags::HIDDEN) {
                    text.push(' ');
                } else {
                    text.push(if cell.c == '\t' { ' ' } else { cell.c });
                    if let Some(marks) = cell.zerowidth() {
                        text.extend(marks);
                    }
                }
                let byte_end = text.len();
                if p == point {
                    target_byte = Some(byte_start);
                }
                cells.push((byte_start, byte_end, p, cell.flags.contains(Flags::WIDE_CHAR)));
            }
        }
        let target_byte = target_byte?;
        let (match_start, match_end, uri) = find_url_at_byte(&text, target_byte)?;
        let first = cells.iter().find(|(_, end, _, _)| *end > match_start)?;
        let last = cells.iter().rev().find(|(start, _, _, _)| *start < match_end)?;
        let mut end = last.2;
        if last.3 && end.column < last_col {
            end.column += 1;
        }
        Some(TerminalLink {
            start: first.2,
            end,
            uri,
            explicit: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::emulator::GridSize;

    fn term(rows: u16, cols: u16, data: &[u8]) -> Emulator {
        let mut em = Emulator::new(GridSize { rows, cols });
        em.process(data);
        em
    }

    #[test]
    fn detects_plain_urls_with_punctuation_brackets_and_cjk_neighbors() {
        let em = term(
            4,
            80,
            "See (https://en.wikipedia.org/wiki/Rust_(programming_language)).\r\n\
             打开https://example.com/a?x=1&y=2，然后继续\r\n\
             │https://localhost:8080/health│ mailto:dev@example.com; www.rust-lang.org!"
                .as_bytes(),
        );
        let wiki = em.link_at(0, 12.).unwrap();
        assert_eq!(
            wiki.uri,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert!(!wiki.explicit);
        assert_eq!(wiki.start, GridPoint::new(Line(0), Column(5)));
        assert_eq!(wiki.end, GridPoint::new(Line(0), Column(61)));
        assert!(em.link_at(0, 4.).is_none());
        assert!(em.link_at(0, 62.).is_none());

        let cjk = em.link_at(1, 10.).unwrap();
        assert_eq!(cjk.uri, "https://example.com/a?x=1&y=2");
        assert_eq!(cjk.start, GridPoint::new(Line(1), Column(4)));

        let box_url = em.link_at(2, 5.).unwrap();
        assert_eq!(box_url.uri, "https://localhost:8080/health");
        assert_eq!(box_url.start, GridPoint::new(Line(2), Column(1)));
        assert_eq!(box_url.end, GridPoint::new(Line(2), Column(29)));

        let mail = em.link_at(2, 35.).unwrap();
        assert_eq!(mail.uri, "mailto:dev@example.com");

        let www = em.link_at(2, 60.).unwrap();
        assert_eq!(www.uri, "https://www.rust-lang.org");
    }

    #[test]
    fn detects_soft_wrapped_urls_across_multiple_rows() {
        let em = term(
            4,
            20,
            b"go https://example.com/very/long/path?token=123456 done",
        );
        let on_first_row = em.link_at(0, 10.).unwrap();
        let on_second_row = em.link_at(1, 5.).unwrap();
        let on_third_row = em.link_at(2, 3.).unwrap();
        assert_eq!(on_first_row, on_second_row);
        assert_eq!(on_second_row, on_third_row);
        assert_eq!(
            on_first_row.uri,
            "https://example.com/very/long/path?token=123456"
        );
        assert_eq!(on_first_row.start, GridPoint::new(Line(0), Column(3)));
        assert_eq!(on_first_row.end, GridPoint::new(Line(2), Column(9)));
        assert!(em.link_at(2, 12.).is_none());
    }

    #[test]
    fn detects_osc8_hyperlinks_including_wide_labels_and_rejects_unsafe_schemes() {
        let em = term(
            3,
            30,
            "A \x1b]8;;https://paxri.dev/docs\x1b\\打开文档\x1b]8;;\x1b\\ B\r\n\
             \x1b]8;;file:///etc/passwd\x1b\\secret\x1b]8;;\x1b\\ \
             \x1b]8;;javascript:alert(1)\x1b\\xss\x1b]8;;\x1b\\"
                .as_bytes(),
        );
        let link = em.link_at(0, 3.).unwrap();
        assert!(link.explicit);
        assert_eq!(link.uri, "https://paxri.dev/docs");
        assert_eq!(link.start, GridPoint::new(Line(0), Column(2)));
        assert_eq!(link.end, GridPoint::new(Line(0), Column(9)));
        assert_eq!(em.link_at(0, 9.), Some(link));
        assert!(em.link_at(0, 1.).is_none());
        assert!(em.link_at(0, 10.).is_none());

        assert!(em.link_at(1, 2.).is_none());
        assert!(em.link_at(1, 9.).is_none());
    }

    #[test]
    fn rejects_invalid_or_unsafe_uris_and_inserts_wrap_breaks() {
        assert!(sanitize_uri("file:///tmp/a").is_none());
        assert!(sanitize_uri("javascript:void(0)").is_none());
        assert!(sanitize_uri("mailto:not-an-email").is_none());
        assert_eq!(
            sanitize_uri("https://example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        let wrapped = wrappable_uri("https://example.com/a/b?c=1&d=2");
        assert!(wrapped.contains('\u{200b}'));
        assert_eq!(wrapped.replace('\u{200b}', ""), "https://example.com/a/b?c=1&d=2");
    }
}
