//! Answer terminal queries when redirected stdio has no local terminal to answer them.
use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    grid::Dimensions,
    term::{Config, Osc52},
    vte::ansi::Processor,
};
use std::{cell::RefCell, rc::Rc};
use tunnel_control::terminal::Size;

#[derive(Clone)]
struct Screen {
    size: Size,
    replies: Rc<RefCell<Vec<u8>>>,
}

impl Dimensions for Screen {
    fn columns(&self) -> usize {
        self.size.cols.into()
    }
    fn screen_lines(&self) -> usize {
        self.size.rows.into()
    }
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
}

impl EventListener for Screen {
    fn send_event(&self, event: Event) {
        let text = match event {
            Event::PtyWrite(text) => text,
            Event::TextAreaSizeRequest(format) => format(WindowSize {
                num_lines: self.size.rows,
                num_cols: self.size.cols,
                cell_height: 0,
                cell_width: 0,
            }),
            _ => return,
        };
        self.replies.borrow_mut().extend_from_slice(text.as_bytes());
    }
}

pub(super) struct RedirectedTerminal {
    parser: Processor,
    term: Term<Screen>,
    screen: Screen,
}

impl RedirectedTerminal {
    pub(super) fn new(size: Size) -> Self {
        let screen = Screen {
            size,
            replies: Default::default(),
        };
        let config = Config {
            scrolling_history: 0,
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let term = Term::new(config, &screen, screen.clone());
        Self {
            parser: Processor::new(),
            term,
            screen,
        }
    }

    pub(super) fn replies(&mut self, data: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, data);
        // There is no rendered frame to defer while an application synchronizes an update.
        self.parser.stop_sync(&mut self.term);
        std::mem::take(&mut *self.screen.replies.borrow_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_initial_and_split_cursor_queries_at_the_actual_position() {
        let mut term = RedirectedTerminal::new(Size { rows: 24, cols: 80 });
        assert_eq!(term.replies(b"\x1b[6n"), b"\x1b[1;1R");
        assert!(term.replies(b"\x1b[5;7Hxy\x1b[").is_empty());
        assert_eq!(term.replies(b"6n"), b"\x1b[5;9R");
    }
}
