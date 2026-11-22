use peerhatch_ui::terminal::cursor;
use peerhatch_ui::terminal::display;
use peerhatch_ui::terminal::emulator;
use peerhatch_ui::terminal::keys;
mod mouse;
mod session;
pub(crate) use peerhatch_ui::terminal::title::TabTitle;

use crate::components::{ButtonVariant::Primary, ScrollMetrics, ScrollSource, Scrollbar, Ui};
use crate::theme::SPACE_2;
use crate::theme::fonts::{CellSize, MonospaceFonts};
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    term::TermMode,
};
use emulator::{Emulator, GridSize};
use gpui::{prelude::*, *};
use session::{Output, Session};
use std::{ops::Range, path::Path};


fn layout_grid(available: Bounds<Pixels>, cell: CellSize) -> (GridSize, Bounds<Pixels>) {
    let grid = GridSize::from_viewport(available.size, cell);
    let cells = size(
        px(f32::from(grid.cols) * cell.width).min(available.size.width),
        px(f32::from(grid.rows) * cell.height).min(available.size.height),
    );
    (grid, Bounds::centered_at(available.center(), cells))
}

pub struct Terminal {
    focus: FocusHandle,
    pub(super) title: Entity<TabTitle>,
    emulator: Emulator,
    session: Option<Session>,
    error: Option<String>,
    can_open_shell: bool,
    marked: cursor::Preedit,
    viewport: Bounds<Pixels>,
    selecting: bool,
    mouse: mouse::Mouse,
    hidden_pointer_at: Option<Point<Pixels>>,
    focused: bool,
    focus_observers: Option<(WindowId, [Subscription; 3])>,
}

pub(super) struct ShellExited;
impl EventEmitter<ShellExited> for Terminal {}

impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScrollSource for Entity<Terminal> {
    fn metrics(&self, cx: &App) -> ScrollMetrics {
        let grid = self.read(cx).emulator.term.grid();
        ScrollMetrics::new(
            grid.screen_lines() as f32,
            grid.history_size() as f32,
            (grid.history_size() - grid.display_offset()) as f32,
        )
    }

    fn scroll_to(&self, offset: f32, cx: &mut App) {
        self.update(cx, |view, cx| {
            let grid = view.emulator.term.grid();
            let from_bottom = grid.history_size().saturating_sub(offset.round() as usize);
            let delta = from_bottom as i32 - grid.display_offset() as i32;
            if delta != 0 {
                view.emulator.scroll(Scroll::Delta(delta));
                cx.notify();
            }
        });
    }
}

impl Terminal {
    pub(super) fn open(
        peer: &str,
        title: TabTitle,
        dashboard: &Entity<crate::Dashboard>,
        runtime: &tokio::runtime::Handle,
        ipc: Option<&Path>,
        window: &Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let appearance = crate::theme::current(cx).terminal;
        let font = cx.global::<MonospaceFonts>().resolve(&appearance);
        let grid = GridSize::from_viewport(window.viewport_size(), font.cell);
        let palette =
            crate::theme::terminal_palette(appearance.scheme, crate::theme::palette(window, cx));
        let (session, events) = Session::start(peer, ipc, runtime);
        let peer = peer.to_owned();
        cx.new(|cx| {
            cx.observe(dashboard, move |view: &mut Self, dashboard, cx| {
                let online = dashboard.read(cx).can_open_shell(&peer);
                if view.can_open_shell != online {
                    view.can_open_shell = online;
                    if !view.is_running() {
                        cx.notify();
                    }
                }
            })
            .detach();
            let focus = cx.focus_handle();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    let Ok(pending) = this.update(cx, |view: &mut Terminal, cx| {
                        view.output(event, cx);
                        // Coalesce ready chunks into the same frame.
                        for event in std::iter::from_fn(|| events.try_recv().ok()).take(32) {
                            view.output(event, cx);
                        }
                        let reply = view.emulator.finish_batch();
                        if let Some(title) = view.emulator.take_title_update() {
                            view.title.update(cx, |tab, cx| {
                                if tab.set_pty(title) {
                                    cx.notify();
                                }
                            });
                        }
                        if !reply.is_empty() {
                            view.send(reply);
                        }
                        cx.notify();
                        view.session.as_ref().and_then(|s| s.input.pending())
                    }) else {
                        break;
                    };
                    // Backpressure a peer producing queries faster than it reads the replies.
                    if let Some(pending) = pending {
                        let _ = pending.recv().await;
                    }
                }
            })
            .detach();
            let mut emulator = Emulator::with_palette(grid, palette, font);
            emulator.set_default_cursor(cursor::default_style(&appearance));
            Terminal {
                focus,
                title: cx.new(|_| title),
                emulator,
                session: Some(session),
                error: None,
                can_open_shell: true,
                marked: cursor::Preedit::default(),
                viewport: Bounds::default(),
                selecting: false,
                mouse: mouse::Mouse::default(),
                hidden_pointer_at: None,
                focused: false,
                focus_observers: None,
            }
        })
    }

    pub(super) fn is_running(&self) -> bool {
        self.session.is_some()
    }

    pub(super) fn has_exited(&self) -> bool {
        self.session.is_none() && self.error.is_none()
    }

    fn resize(&mut self, available: Bounds<Pixels>, cx: &mut Context<Self>) -> Bounds<Pixels> {
        let (grid, bounds) = layout_grid(available, self.emulator.font.cell);
        self.viewport = bounds;
        if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
            if let Some(session) = &mut self.session {
                session.resize(grid.pty(self.emulator.font.cell));
            }
            if self.emulator.resize(grid) {
                cx.notify();
            }
        }
        bounds
    }
    fn output(&mut self, event: Output, cx: &mut Context<Self>) {
        match event {
            Output::Bytes(bytes) => self.emulator.process(&bytes),
            Output::Error(error) => {
                if self.is_running() {
                    self.error = Some(format!("Terminal I/O: {error}"));
                }
            }
            Output::Exited(result) => {
                self.session = None;
                match result {
                    Ok(_) => {
                        self.error = None;
                        cx.emit(ShellExited);
                    }
                    Err(e) => self.error = Some(format!("Session error: {e}")),
                }
            }
        }
    }
    fn cell_at(&self, position: Point<Pixels>) -> (usize, f32) {
        let local = position - self.viewport.origin;
        let last = self.emulator.term.screen_lines() - 1;
        let cell = self.emulator.font.cell;
        let row = (f32::from(local.y) / cell.height)
            .floor()
            .clamp(0., last as f32) as usize;
        (row, f32::from(local.x) / cell.width)
    }

    fn start_selection(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(w);
        let (row, x) = self.cell_at(e.position);
        if e.modifiers.shift && self.emulator.term.selection.is_some() {
            self.emulator.extend_selection(row, x);
        } else {
            self.emulator.start_selection(row, x, e.click_count);
        }
        self.selecting = true;
        cx.notify();
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (row, x) = self.cell_at(position);
        self.emulator.extend_selection(row, x);
        cx.notify();
    }

    fn send(&mut self, bytes: Vec<u8>) {
        if let Some(session) = &mut self.session {
            session.input.send(bytes);
        }
    }

    fn focus_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = window.is_window_active() && self.focus.is_focused(window);
        if focused != self.focused {
            self.focused = focused;
            if !focused {
                #[cfg(windows)]
                if !self.marked.text.is_empty() {
                    crate::ime::blur(window);
                }
                self.marked.clear();
                self.selecting = false;
                self.hidden_pointer_at = None;
                self.release_pointer();
            }
            if self.emulator.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                self.send(if focused { b"\x1b[I" } else { b"\x1b[O" }.to_vec());
            }
            cx.notify();
        }
    }

    fn observe_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = window.window_handle().window_id();
        if self
            .focus_observers
            .as_ref()
            .is_none_or(|(old, _)| *old != id)
        {
            self.hidden_pointer_at = None;
            self.marked.clear();
            self.focus_observers = Some((
                id,
                [
                    cx.on_focus(&self.focus, window, Self::focus_changed),
                    cx.on_blur(&self.focus, window, Self::focus_changed),
                    cx.observe_window_activation(window, Self::focus_changed),
                ],
            ));
            self.focus_changed(window, cx);
        }
    }

    fn typing_pointer_position(&self, window: &Window) -> Option<Point<Pixels>> {
        let position = window.mouse_position();
        (self.is_running()
            && self.focus.is_focused(window)
            && window.is_window_active()
            && crate::chrome::pointer_over_window(window)
            && Bounds::new(Point::default(), window.viewport_size()).contains(&position))
        .then_some(position)
    }

    fn write(&mut self, bytes: Vec<u8>, window: &Window) {
        if self.is_running() && !bytes.is_empty() {
            self.hidden_pointer_at = self.typing_pointer_position(window);
            self.emulator.input_started();
            self.emulator.term.selection = None;
            self.selecting = false;
            self.emulator.scroll(Scroll::Bottom);
            self.send(bytes);
        }
    }
    fn paste(&mut self, window: &Window, cx: &App) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let text = text.replace("\r\n", "\n").replace('\n', "\r");
            let bytes = if self
                .emulator
                .term
                .mode()
                .contains(TermMode::BRACKETED_PASTE)
            {
                format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
            } else {
                text.into_bytes()
            };
            self.write(bytes, window);
        }
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let shortcut = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control && key.modifiers.shift
        };
        if self.can_open_shell && !self.is_running() && key.key == "enter" && !key.modifiers.modified()
        {
            window.dispatch_action(Box::new(super::workspace::ReconnectTerminal), cx);
        } else if shortcut && key.key.eq_ignore_ascii_case("v") {
            self.paste(window, cx);
        } else if key.key.eq_ignore_ascii_case("c")
            && (shortcut
                || (!cfg!(target_os = "macos")
                    && key.modifiers == Modifiers::control()
                    && self.emulator.selected_range().is_some()))
        {
            let text = self
                .emulator
                .selected_text()
                .unwrap_or_else(|| self.emulator.visible_text());
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        } else if let Some(bytes) = keys::sequence(
            key,
            self.emulator.term.mode().contains(TermMode::APP_CURSOR),
        ) {
            self.write(bytes, window);
        } else {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        cx.notify();
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let appearance = crate::theme::current(cx).terminal;
        self.emulator.font = cx.global::<MonospaceFonts>().resolve(&appearance);
        let palette = crate::theme::terminal_palette(appearance.scheme, ui.colors);
        self.emulator.palette = palette;
        let background = display::color(
            alacritty_terminal::vte::ansi::Color::Named(
                alacritty_terminal::vte::ansi::NamedColor::Background,
            ),
            palette,
            self.emulator.term.colors(),
        );
        self.observe_focus(window, cx);
        if self.hidden_pointer_at.is_some()
            && self.hidden_pointer_at != self.typing_pointer_position(window)
        {
            self.hidden_pointer_at = None;
        }
        let hide_pointer = self.hidden_pointer_at.is_some();
        self.emulator
            .set_default_cursor(cursor::default_style(&appearance));
        let style = self.emulator.term.cursor_style();
        let blink =
            style.blinking && self.focused && self.marked.text.is_empty() && self.is_running();
        let cursor_on = cursor::blink_visible(
            blink,
            cursor::CursorCell::visible(&self.emulator),
            window,
            cx,
        );
        let entity = cx.entity();
        let sizing = entity.clone();
        let focus = self.focus.clone();
        let screen = canvas(
            move |bounds, _, cx| sizing.update(cx, |view, cx| view.resize(bounds, cx)),
            move |_, bounds, window, cx| {
                if hide_pointer {
                    window.set_window_cursor_style(CursorStyle::None);
                }
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
                entity.update(cx, |view, cx| {
                    display::paint(
                        &view.emulator,
                        bounds,
                        (!view.marked.text.is_empty()).then_some(&view.marked),
                        cursor_on.then_some(style.shape),
                        palette,
                        window,
                        cx,
                    )
                });
                let moving = entity.downgrade();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let _ = moving.update(cx, |view, cx| {
                        if view.hidden_pointer_at.is_some_and(|at| at != e.position) {
                            view.hidden_pointer_at = None;
                            cx.notify();
                        }
                        view.mouse_drag(e, cx);
                    });
                });
                let release = entity.downgrade();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = release.update(cx, |v, cx| v.mouse_up(e, cx));
                    }
                });
            },
        )
        .size_full();
        div()
            .id("device-terminal")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(background))
            .text_color(rgb(palette.surface.foreground))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .flex()
                        .gap_2()
                        .items_center()
                        .bg(rgb(ui.colors.canvas.background))
                        .text_color(rgb(ui.colors.canvas.foreground))
                        .child(div().flex_1().text_sm().child(error))
                        .when(self.can_open_shell && !self.is_running(), |bar| {
                            bar.child(ui.button(Primary, "reconnect", "Reconnect").on_click(
                                |_, w, cx| {
                                    w.dispatch_action(
                                        Box::new(super::workspace::ReconnectTerminal),
                                        cx,
                                    );
                                },
                            ))
                        }),
                )
            })
            .child(
                div()
                    .id("terminal-screen")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .p(SPACE_2)
                    .overflow_hidden()
                    .cursor(CursorStyle::Arrow)
                    .on_any_mouse_down(cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_hover))
                    .on_scroll_wheel(cx.listener(Self::wheel))
                    .child(screen)
                    .child(Scrollbar::vertical("terminal-history", cx.entity())),
            )
    }
}

// Only IME composition belongs to the local editor. Committed text is sent to
// the PTY, whose echo and cursor are controlled by the foreground application.
impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<_> = self.marked.text.encode_utf16().collect();
        let selected = utf16.get(range.clone())?;
        *actual = Some(range);
        Some(String::from_utf16_lossy(selected))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.marked.selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.text.is_empty()).then(|| 0..self.marked.text.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !window.is_window_active() || !self.focus.is_focused(window) {
            return;
        }
        self.marked.clear();
        if !text.is_empty() {
            self.write(text.as_bytes().to_vec(), window);
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !window.is_window_active() || !self.focus.is_focused(window) {
            return;
        }
        if !text.is_empty() {
            self.hidden_pointer_at = self.typing_pointer_position(window);
            self.emulator.scroll(Scroll::Bottom);
        }
        self.marked.replace(range, text, selection);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(
            self.marked
                .layout(&self.emulator, bounds.origin)?
                .bounds_for_range(range),
        )
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod layout_tests {
    use super::{CellSize, layout_grid};
    const CELL: CellSize = CellSize {
        width: 9.,
        height: 20.,
    };
    use gpui::{Bounds, point, px, size};

    #[test]
    fn whole_cells_leave_equal_space_on_opposite_edges() {
        for height in [20., 39.5, 40., 691., 700., 719.5, 720., 730.] {
            let available = Bounds::new(point(px(8.), px(48.)), size(px(951.5), px(height)));
            let (grid, bounds) = layout_grid(available, CELL);
            assert_eq!(grid.pty(CELL).rows, (height / CELL.height).floor() as u16);
            assert_eq!(grid.cols, 105);
            assert_eq!(
                bounds.size,
                size(
                    px(f32::from(grid.cols) * CELL.width),
                    px(f32::from(grid.rows) * CELL.height)
                )
            );
            assert_eq!(bounds.center(), available.center());
        }
    }

    #[test]
    fn a_pane_shorter_than_one_row_keeps_its_input_bounds_inside_the_pane() {
        for height in [0., 7.5] {
            let available = Bounds::new(point(px(5.), px(40.)), size(px(90.), px(height)));
            let (grid, bounds) = layout_grid(available, CELL);
            assert_eq!(grid.rows, 1);
            assert_eq!(bounds, available);
        }
    }
}
