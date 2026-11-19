mod display;
mod keys;
mod selection;
mod session;

use anyhow::Result;
use display::{BACKGROUND, COL, LINE};
use gpui::{prelude::*, *};
use portable_pty::PtySize;
use selection::{Selection, top_row};
use session::{Output, Session};
use std::{num::NonZeroU16, ops::Range, path::Path};

const PAD: f32 = 12.;

fn dimensions(viewport: Size<Pixels>) -> PtySize {
    PtySize {
        cols: (f32::from(viewport.width) / COL).clamp(1., 500.) as u16,
        rows: (f32::from(viewport.height) / LINE).clamp(1., 250.) as u16,
        pixel_width: COL as u16,
        pixel_height: LINE as u16,
    }
}

// Some full-screen programs query the cursor position before painting.
#[derive(Default)]
struct Replies {
    bytes: Vec<u8>,
    discarded: [usize; 2],
}
impl vt100::Callbacks for Replies {
    fn on_scroll(&mut self, _: vt100::capture::RowContents<'_>, alternate: bool) {
        self.discarded[usize::from(alternate)] += 1;
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        end: char,
    ) {
        if first.is_some() || second.is_some() {
            return;
        }
        match (
            end,
            params.first().and_then(|p| p.first()).copied().unwrap_or(0),
        ) {
            ('n', 6) => {
                let (r, c) = screen.cursor_position();
                self.bytes
                    .extend_from_slice(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
            }
            ('n', 5) => self.bytes.extend_from_slice(b"\x1b[0n"),
            ('c', 0) => self.bytes.extend_from_slice(b"\x1b[?1;2c"),
            _ => {}
        }
    }
}

pub struct Terminal {
    focus: FocusHandle,
    parser: vt100::Parser<Replies>,
    session: Option<Session>,
    status: String,
    marked: String,
    viewport: Bounds<Pixels>,
    selection: Option<Selection>,
}

impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Terminal {
    pub(super) fn open(
        peer: &str,
        ipc: Option<&Path>,
        viewport: Size<Pixels>,
        cx: &mut App,
    ) -> Result<Entity<Self>> {
        let grid = dimensions(viewport);
        let (session, events) = Session::start(peer, ipc, grid)?;
        Ok(cx.new(|cx| {
            let focus = cx.focus_handle();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this
                        .update(cx, |view: &mut Terminal, cx| {
                            view.output(event);
                            // Coalesce ready chunks into the same frame.
                            for event in std::iter::from_fn(|| events.try_recv().ok()).take(32) {
                                view.output(event);
                            }
                            let reply = std::mem::take(&mut view.parser.callbacks_mut().bytes);
                            if !reply.is_empty() {
                                view.send(reply);
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            Terminal {
                focus,
                parser: vt100::Parser::new_with_callbacks(
                    grid.rows.try_into().unwrap(),
                    grid.cols.try_into().unwrap(),
                    2000,
                    Replies::default(),
                ),
                session: Some(session),
                status: String::new(),
                marked: String::new(),
                viewport: Bounds::default(),
                selection: None,
            }
        }))
    }

    pub(super) fn is_running(&self) -> bool {
        self.session.is_some()
    }

    fn resize(&mut self, bounds: Bounds<Pixels>) {
        self.viewport = bounds;
        let grid = dimensions(bounds.size);
        let rows = NonZeroU16::new(grid.rows).unwrap();
        let cols = NonZeroU16::new(grid.cols).unwrap();
        if self.parser.screen().size() != (rows, cols) {
            if self.parser.screen().size().1 != cols {
                self.selection = None;
            }
            self.parser.set_size(rows, cols);
            if let Some(session) = &self.session {
                session.resize(grid);
            }
        }
    }
    fn output(&mut self, event: Output) {
        match event {
            Output::Bytes(bytes) => {
                let alternate = self.parser.screen().alternate_screen();
                self.parser.process(&bytes);
                if self.parser.screen().alternate_screen() != alternate
                    || self
                        .selection
                        .as_ref()
                        .is_some_and(|s| s.evicted(self.discarded()))
                {
                    self.selection = None;
                }
            }
            Output::Error(error) => {
                if self.is_running() {
                    self.status = format!("Terminal I/O: {error}");
                }
            }
            Output::Exited(result) => {
                self.session = None;
                self.status = match result {
                    Ok(status) if status.success() => "Session ended".into(),
                    Ok(status) => format!("Disconnected · {status}"),
                    Err(e) => format!("Session error: {e}"),
                };
            }
        }
    }
    fn discarded(&self) -> usize {
        self.parser.callbacks().discarded[usize::from(self.parser.screen().alternate_screen())]
    }

    fn cell_at(&self, position: Point<Pixels>) -> (u16, f32) {
        let local = position - self.viewport.origin;
        let rows = self.parser.screen().size().0.get();
        let row = (f32::from(local.y) / LINE)
            .floor()
            .clamp(0., f32::from(rows - 1)) as u16;
        (row, f32::from(local.x) / COL)
    }

    fn start_selection(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(w);
        let (row, x) = self.cell_at(e.position);
        let screen = self.parser.screen();
        let top = top_row(screen, self.discarded());
        if e.modifiers.shift
            && let Some(selection) = &mut self.selection
        {
            selection.dragging = true;
            selection.extend(screen, top, row, x);
        } else {
            self.selection = Some(Selection::start(screen, top, row, x, e.click_count));
        }
        cx.notify();
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (row, x) = self.cell_at(position);
        let discarded = self.discarded();
        let screen = self.parser.screen();
        if let Some(selection) = &mut self.selection {
            selection.extend(screen, top_row(screen, discarded), row, x);
            cx.notify();
        }
    }

    fn scroll(&mut self, delta: isize) {
        let screen = self.parser.screen_mut();
        if !screen.alternate_screen() {
            screen.set_scrollback(screen.scrollback().saturating_add_signed(delta));
        }
    }

    fn send(&mut self, bytes: Vec<u8>) {
        if let Some(session) = &self.session
            && session.input.try_send(bytes).is_err()
        {
            self.status = "Terminal input is busy".into();
        }
    }

    fn write(&mut self, bytes: Vec<u8>) {
        if self.is_running() {
            self.selection = None;
            self.parser.screen_mut().set_scrollback(0);
            self.send(bytes);
        }
    }
    fn paste(&mut self, cx: &App) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let text = text.replace("\r\n", "\n").replace('\n', "\r");
            let bytes = if self.parser.screen().bracketed_paste() {
                format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
            } else {
                text.into_bytes()
            };
            self.write(bytes);
        }
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let shortcut = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control && key.modifiers.shift
        };
        if !self.is_running() && key.key == "enter" && !key.modifiers.modified() {
            window.dispatch_action(Box::new(super::workspace::ReconnectTerminal), cx);
        } else if shortcut && key.key.eq_ignore_ascii_case("v") {
            self.paste(cx);
        } else if key.key.eq_ignore_ascii_case("c")
            && (shortcut
                || (!cfg!(target_os = "macos")
                    && key.modifiers == Modifiers::control()
                    && self.selection.as_ref().is_some_and(|s| !s.is_empty())))
        {
            let discarded = self.discarded();
            let text = self
                .selection
                .as_ref()
                .and_then(|s| s.text(self.parser.screen_mut(), discarded))
                .unwrap_or_else(|| self.parser.screen().contents());
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        } else if let Some(bytes) = keys::sequence(key, self.parser.screen().application_cursor()) {
            self.write(bytes);
        } else {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        cx.notify();
    }
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus.clone();
        let screen = canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
                entity.update(cx, |view, cx| {
                    view.resize(bounds);
                    let screen = view.parser.screen();
                    display::paint(
                        screen,
                        bounds,
                        &view.marked,
                        view.selection.as_ref(),
                        top_row(screen, view.discarded()),
                        window,
                        cx,
                    )
                });
                let moving = entity.downgrade();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let _ = moving.update(cx, |v, cx| {
                        if !v.selection.as_ref().is_some_and(|s| s.dragging) {
                            return;
                        }
                        if e.pressed_button != Some(MouseButton::Left) {
                            if let Some(s) = &mut v.selection {
                                s.dragging = false;
                            }
                            return;
                        }
                        if e.position.y < v.viewport.top() {
                            v.scroll(1);
                        }
                        if e.position.y >= v.viewport.bottom() {
                            v.scroll(-1);
                        }
                        v.extend_selection(e.position, cx);
                    });
                });
                let release = entity.downgrade();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if e.button == MouseButton::Left && phase == DispatchPhase::Capture {
                        let _ = release.update(cx, |v, cx| {
                            if let Some(s) = &mut v.selection
                                && s.dragging
                            {
                                s.dragging = false;
                                v.extend_selection(e.position, cx);
                            }
                        });
                    }
                });
            },
        )
        .size_full();
        div()
            .id("ssh-terminal")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(0xddeee4))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .when(!self.status.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .flex()
                        .gap_2()
                        .items_center()
                        .bg(rgb(super::SURFACE))
                        .text_color(rgb(super::INK))
                        .child(div().flex_1().text_sm().child(self.status.clone()))
                        .when(!self.is_running(), |bar| {
                            bar.child(super::primary("reconnect", "Reconnect").on_click(
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
                    .flex_1()
                    .min_h_0()
                    .p(px(PAD))
                    .overflow_hidden()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::start_selection))
                    .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                        let delta = f32::from(e.delta.pixel_delta(px(LINE)).y) / LINE;
                        v.scroll(delta.round() as isize);
                        if v.selection.as_ref().is_some_and(|s| s.dragging) {
                            v.extend_selection(e.position, cx);
                        }
                        cx.notify();
                    }))
                    .child(screen),
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
        let utf16: Vec<_> = self.marked.encode_utf16().collect();
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
        let end = self.marked.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        if !text.is_empty() {
            self.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = text.into();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (r, c) = self.parser.screen().cursor_position();
        Some(Bounds::new(
            bounds.origin + point(px(f32::from(c) * COL), px(f32::from(r) * LINE)),
            size(px(COL), px(LINE)),
        ))
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
