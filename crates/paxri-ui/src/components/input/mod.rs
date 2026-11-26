//! Native text inputs share the clipboard, selection, IME and cursor implementation.
mod buffer;
mod layout;
use crate::components::byte_at_utf16;
use buffer::TextBuffer;
use layout::{CARET_SPACE, InputLayout};

use super::{ButtonVariant::Ghost, Icon, Ui};
use crate::theme;
use gpui::{prelude::*, *};
use std::{
    ops::Range,
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;

pub type Input = TextInput<false>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NumberFormat {
    Decimal,
    Integer,
    SignedInteger,
}

impl NumberFormat {
    fn permits(self, value: &str) -> bool {
        value.chars().all(|c| {
            c.is_ascii_digit()
                || match self {
                    Self::Decimal => c == '.',
                    Self::SignedInteger => c == '-',
                    Self::Integer => false,
                }
        })
    }
}

pub struct NumberStep(pub i32);
impl<const MULTILINE: bool> EventEmitter<NumberStep> for TextInput<MULTILINE> {}

pub struct EditingFinished;
#[cfg(target_os = "android")]
pub struct KeyboardRequested;
#[cfg(target_os = "android")]
impl<const MULTILINE: bool> EventEmitter<KeyboardRequested> for TextInput<MULTILINE> {}
impl<const MULTILINE: bool> EventEmitter<EditingFinished> for TextInput<MULTILINE> {}

struct SelectionDrag {
    pointer: Point<Pixels>,
    last_frame: Option<Instant>,
}

pub struct TextInput<const MULTILINE: bool> {
    buffer: TextBuffer<MULTILINE>,
    focus: FocusHandle,
    blur_subscription: Option<(WindowId, Subscription)>,
    placeholder: String,
    search: bool,
    number: Option<(NumberFormat, &'static str)>,
    wheel_remainder: f32,
    #[cfg(target_os = "android")]
    drag_steps: Option<i32>,
    commit_on_finish: bool,
    inline: bool,
    layout: Option<InputLayout>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Point<Pixels>,
    reveal_cursor: bool,
    drag: Option<SelectionDrag>,
    row_hint: Option<usize>,
    preferred_x: Option<Pixels>,
}

impl<const MULTILINE: bool> Focusable for TextInput<MULTILINE> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput<false> {
    pub fn setting(placeholder: &str, cx: &mut App) -> Entity<Self> {
        let input = Self::new("", placeholder, cx);
        input.update(cx, |field, _| field.commit_on_finish = true);
        input
    }

    pub fn number(format: NumberFormat, unit: &'static str, cx: &mut App) -> Entity<Self> {
        let field = Self::setting("", cx);
        field.update(cx, |field, _| field.number = Some((format, unit)));
        field
    }

    pub fn search(placeholder: &str, cx: &mut App) -> Entity<Self> {
        let field = Self::new("", placeholder, cx);
        field.update(cx, |field, _| {
            field.search = true;
            field.commit_on_finish = cfg!(target_os = "android");
        });
        field
    }

    pub fn inline(text: &str, cx: &mut App) -> Entity<Self> {
        let mut buffer = TextBuffer::new(text);
        buffer.anchor = 0;
        Self::with_buffer(buffer, "", true, cx)
    }

    #[cfg(target_os = "android")]
    pub fn text_bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
    }

    #[cfg(target_os = "android")]
    pub fn edit_at_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.buffer.cursor = self.buffer.text.len();
        self.buffer.anchor = self.buffer.cursor;
        self.reveal_cursor = true;
        self.focus.focus(window, cx);
        cx.emit(KeyboardRequested);
        cx.notify();
    }

    pub fn is_composing(&self) -> bool {
        self.buffer.marked.is_some()
    }
}

impl<const MULTILINE: bool> TextInput<MULTILINE> {
    pub fn new(text: &str, placeholder: &str, cx: &mut App) -> Entity<Self> {
        Self::with_buffer(TextBuffer::new(text), placeholder, false, cx)
    }

    pub fn read_only(text: &str, cx: &mut App) -> Entity<Self> {
        Self::with_buffer(TextBuffer::read_only(text), "", false, cx)
    }

    fn with_buffer(
        buffer: TextBuffer<MULTILINE>,
        placeholder: &str,
        inline: bool,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self {
            placeholder: placeholder.into(),
            search: false,
            number: None,
            wheel_remainder: 0.,
            #[cfg(target_os = "android")]
            drag_steps: None,
            commit_on_finish: false,
            inline,
            focus: cx.focus_handle().tab_stop(true),
            blur_subscription: None,
            buffer,
            layout: None,
            bounds: None,
            scroll: Point::default(),
            reveal_cursor: true,
            drag: None,
            row_hint: None,
            preferred_x: None,
        })
    }

    pub fn text(&self) -> &str {
        &self.buffer.text
    }

    pub fn set_placeholder(&mut self, placeholder: &str, cx: &mut Context<Self>) {
        if self.placeholder != placeholder {
            self.placeholder = placeholder.to_owned();
            cx.notify();
        }
    }

    fn is_focused(&self, window: &Window) -> bool {
        window.is_window_active() && self.focus.is_focused(window)
    }

    fn observe_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = window.window_handle().window_id();
        if self
            .blur_subscription
            .as_ref()
            .is_some_and(|(old, _)| *old == id)
        {
            return;
        }
        let subscription = cx.on_blur(&self.focus, window, |field, _window, cx| {
            field.drag = None;
            field.wheel_remainder = 0.;
            #[cfg(target_os = "android")]
            {
                field.drag_steps = None;
            }
            if field.buffer.cancel_composition() {
                #[cfg(windows)]
                crate::ime::blur(_window);
                field.after_edit();
                cx.notify();
            }
            if field.commit_on_finish {
                cx.emit(EditingFinished);
            }
        });
        self.blur_subscription = Some((id, subscription));
    }

    /// Keep focus, selection and scrolling when a refreshed model still contains the same value.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.text() == text {
            return false;
        }
        self.buffer = if self.buffer.read_only {
            TextBuffer::read_only(text)
        } else {
            TextBuffer::new(text)
        };
        self.layout = None;
        self.drag = None;
        self.scroll = Point::default();
        self.row_hint = None;
        self.preferred_x = None;
        self.reveal_cursor = true;
        cx.notify();
        true
    }

    fn replace(&mut self, range: Option<Range<usize>>, value: &str) {
        if self
            .number
            .is_some_and(|(format, _)| !format.permits(value))
        {
            return;
        }
        self.buffer.replace(range, value);
        self.after_edit();
    }

    #[cfg(target_os = "android")]
    pub fn insert_hardware_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.buffer.read_only && !text.is_empty() && !text.chars().any(char::is_control) {
            self.replace(None, text);
            cx.notify();
        }
    }

    fn step_button(&self, up: bool, ui: Ui, cx: &Context<Self>) -> Stateful<Div> {
        let icon = if up {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        };
        let button = div()
            .id(if up { "increment" } else { "decrement" })
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .rounded(px(3.))
            .hover(|button| button.bg(rgb(ui.colors.secondary.background)))
            .active(|button| button.bg(rgb(ui.colors.border)))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(NumberStep(if up { 1 } else { -1 }));
                cx.stop_propagation();
            }))
            .child(icon.draw(12.));
        #[cfg(target_os = "android")]
        let button = button
            .role(Role::Button)
            .aria_label(if up { "Increase" } else { "Decrease" });
        button
    }

    fn number_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_focused(window) {
            return;
        }
        let delta = match event.delta {
            ScrollDelta::Lines(lines) => lines.y.clamp(-1., 1.),
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 24.,
        };
        if delta == 0. {
            return;
        }
        cx.stop_propagation();
        self.wheel_remainder += delta;
        let steps = self.wheel_remainder.trunc() as i32;
        if steps != 0 {
            self.wheel_remainder -= steps as f32;
            cx.emit(NumberStep(steps));
        }
    }

    #[cfg(target_os = "android")]
    fn number_drag(
        &mut self,
        event: &TouchDragEvent,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.phase {
            TouchPhase::Started => {
                // Leave the stepper strip and all unfocused drags to normal tap/page handling.
                if !self.is_focused(window)
                    || !bounds.contains(&event.start_position)
                    || event.start_position.x >= bounds.right() - px(28.)
                {
                    return;
                }
                window.prevent_default();
                self.drag_steps = Some(0);
            }
            TouchPhase::Moved | TouchPhase::Ended => {
                let Some(previous) = self.drag_steps else {
                    return;
                };
                let delta = event.start_position - event.position;
                let steps = (f32::from(delta.y) / 16.).trunc() as i32;
                if steps != previous {
                    cx.emit(NumberStep(steps - previous));
                }
                self.drag_steps = (event.phase == TouchPhase::Moved).then_some(steps);
                if event.phase == TouchPhase::Ended && delta.magnitude() < 4. {
                    self.point_cursor(event.position);
                    self.buffer.anchor = self.buffer.cursor;
                    cx.emit(KeyboardRequested);
                    cx.notify();
                }
            }
            TouchPhase::Cancelled => self.drag_steps = None,
        }
    }

    fn after_edit(&mut self) {
        self.drag = None;
        self.layout = None;
        self.row_hint = None;
        self.preferred_x = None;
        self.reveal_cursor = true;
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_focused(window) || self.buffer.marked.is_some() {
            return;
        }
        let key = &ev.keystroke;
        let shortcut = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control
        };
        let sel = self.buffer.selection();
        let vertical = MULTILINE && matches!(key.key.as_str(), "up" | "down");
        let row_key = MULTILINE && matches!(key.key.as_str(), "up" | "down" | "home" | "end");
        if row_key
            && self.layout.is_none()
            && let Some(bounds) = self.bounds
        {
            self.layout = self.shape(bounds, window, cx);
        }
        if !vertical {
            self.preferred_x = None;
        }
        match key.key.as_str() {
            "backspace" | "delete" | "enter" if self.buffer.read_only => {}
            "x" | "v" if shortcut && self.buffer.read_only => {}
            #[cfg(target_os = "android")]
            "enter" | "escape" if self.commit_on_finish => window.blur(cx),
            #[cfg(not(target_os = "android"))]
            "enter" if self.commit_on_finish => cx.emit(EditingFinished),
            "up" | "down" if self.number.is_some() => {
                cx.emit(NumberStep(if key.key == "up" { 1 } else { -1 }));
            }
            "enter" if MULTILINE => {
                self.replace(None, "\n");
            }
            "up" | "down" | "home" | "end" if row_key => {
                if let Some(layout) = &self.layout {
                    let row = layout.row_at(self.buffer.cursor, self.row_hint);
                    let (byte, target) = match key.key.as_str() {
                        "home" if shortcut => (0, 0),
                        "end" if shortcut => (self.buffer.text.len(), layout.row_count() - 1),
                        "home" => (layout.range(row).start, row),
                        "end" => (layout.range(row).end, row),
                        _ => {
                            let x = *self
                                .preferred_x
                                .get_or_insert_with(|| layout.x(row, self.buffer.cursor));
                            let target = if key.key == "up" {
                                row.saturating_sub(1)
                            } else {
                                (row + 1).min(layout.row_count() - 1)
                            };
                            (layout.byte_at(target, x), target)
                        }
                    };
                    self.buffer.cursor = byte.min(self.buffer.text.len());
                    self.row_hint = Some(target);
                    if !key.modifiers.shift {
                        self.buffer.anchor = self.buffer.cursor;
                    }
                }
            }
            "a" if shortcut => {
                self.buffer.anchor = 0;
                self.buffer.cursor = self.buffer.text.len();
            }
            "c" | "x" if shortcut => {
                if !sel.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.text[sel].to_owned(),
                    ));
                    if key.key == "x" {
                        self.replace(None, "");
                    }
                }
            }
            "v" if shortcut => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.replace(None, &t);
                }
            }
            "left" | "right" | "home" | "end" | "backspace" | "delete" => {
                let prev = self.buffer.text[..self.buffer.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map_or(0, |(i, _)| i);
                let next = self.buffer.cursor
                    + self.buffer.text[self.buffer.cursor..]
                        .graphemes(true)
                        .next()
                        .map_or(0, str::len);
                match key.key.as_str() {
                    "backspace" | "delete" => {
                        if sel.is_empty() {
                            self.buffer.anchor = if key.key == "backspace" { prev } else { next };
                        }
                        self.replace(None, "");
                    }
                    k => {
                        self.buffer.cursor = match k {
                            "home" => 0,
                            "end" => self.buffer.text.len(),
                            "left" if !key.modifiers.shift && !sel.is_empty() => sel.start,
                            "right" if !key.modifiers.shift && !sel.is_empty() => sel.end,
                            "left" => prev,
                            _ => next,
                        };
                        if !key.modifiers.shift {
                            self.buffer.anchor = self.buffer.cursor;
                        }
                    }
                }
            }
            _ => return,
        }
        if !row_key {
            self.row_hint = None;
        }
        self.drag = None;
        self.reveal_cursor = true;
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn hit(&self, pt: Point<Pixels>) -> (usize, Option<usize>) {
        self.layout
            .as_ref()
            .map_or((self.buffer.cursor, self.row_hint), |layout| {
                let (byte, row) = layout.hit(pt);
                (byte.min(self.buffer.text.len()), Some(row))
            })
    }

    fn point_cursor(&mut self, pt: Point<Pixels>) {
        (self.buffer.cursor, self.row_hint) = self.hit(pt);
        self.reveal_cursor = true;
        self.preferred_x = None;
    }

    fn move_selection(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) || !self.is_focused(window) {
            self.drag = None;
            return;
        }
        let Some(drag) = &mut self.drag else { return };
        if drag.pointer != event.position {
            drag.pointer = event.position;
            drag.last_frame.get_or_insert_with(Instant::now);
            self.reveal_cursor = false;
            cx.notify();
        }
    }

    fn end_selection(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        if let Some(drag) = self.drag.take()
            && (drag.last_frame.is_some() || drag.pointer != event.position)
            && self.is_focused(window)
            && let Some(layout) = &mut self.layout
        {
            let (byte, row, _) = layout.drag_hit::<MULTILINE>(event.position, Duration::ZERO);
            self.buffer.cursor = byte.min(self.buffer.text.len());
            self.row_hint = Some(row);
            cx.notify();
        }
    }

    fn shape(&self, bounds: Bounds<Pixels>, window: &Window, cx: &App) -> Option<InputLayout> {
        let colors = theme::palette(window, cx);
        let empty = self.buffer.text.is_empty();
        let text: SharedString = (if empty {
            &self.placeholder
        } else {
            &self.buffer.text
        })
        .clone()
        .into();
        let run = TextRun {
            len: text.len(),
            font: window.text_style().font(),
            color: rgb(if empty {
                colors.muted_foreground
            } else {
                colors.canvas.foreground
            })
            .into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = MULTILINE.then(|| (bounds.size.width - px(2.)).max(px(1.)));
        let lines = window
            .text_system()
            .shape_text(text, rems(0.875).to_pixels(window.rem_size()), &[run], width, None)
            .ok()?;
        let height = if MULTILINE {
            rems(1.25).to_pixels(window.rem_size())
        } else {
            bounds.size.height.max(px(1.))
        };
        Some(InputLayout::new(lines.into_vec(), bounds, height))
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, caret_on: bool, window: &mut Window, cx: &mut App) {
        let Some(mut layout) = self.shape(bounds, window, cx) else {
            return;
        };
        let colors = theme::palette(window, cx);
        let focused = self.is_focused(window);
        layout.scroll = self.scroll;
        if self.reveal_cursor || self.bounds.is_some_and(|old| old.size != bounds.size) {
            let row = layout.row_at(self.buffer.cursor, self.row_hint);
            if MULTILINE {
                let y = layout.height * row;
                layout.scroll.y = layout
                    .scroll
                    .y
                    .min(y)
                    .max(y + layout.height - bounds.size.height);
            } else {
                let x = layout.x(row, self.buffer.cursor);
                layout.scroll.x = layout
                    .scroll
                    .x
                    .min(x)
                    .max(x + CARET_SPACE - bounds.size.width);
            }
        }
        if MULTILINE {
            layout.scroll.y = layout.scroll.y.clamp(px(0.), layout.max_scroll_y());
        } else {
            layout.scroll.x = layout.scroll.x.clamp(px(0.), layout.max_scroll_x());
        }
        if focused
            && let Some(drag) = &mut self.drag
            && let Some(last) = &mut drag.last_frame
        {
            let now = Instant::now();
            let (byte, row, moving) = layout.drag_hit::<MULTILINE>(drag.pointer, now - *last);
            *last = now;
            self.buffer.cursor = byte.min(self.buffer.text.len());
            self.row_hint = Some(row);
            if moving {
                window.request_animation_frame();
            }
        }
        self.scroll = layout.scroll;
        self.reveal_cursor = false;
        self.bounds = Some(bounds);
        let selection = self.buffer.selection();
        if focused {
            layout.paint_range(selection.clone(), false, colors.selection, window);
        }
        layout.paint_text(window, cx);
        if focused && caret_on && selection.is_empty() {
            window.paint_quad(fill(
                layout.caret_bounds(self.buffer.cursor, self.row_hint),
                rgb(colors.ring),
            ));
        }
        if focused && let Some(r) = &self.buffer.marked {
            layout.paint_range(r.clone(), true, colors.ring, window);
        }
        self.layout = Some(layout);
    }
}

impl<const MULTILINE: bool> Render for TextInput<MULTILINE> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.observe_blur(window, cx);
        let field = cx.entity();
        let editable = !self.buffer.read_only;
        let blink = editable
            && self.is_focused(window)
            && self.buffer.selection().is_empty()
            && self.buffer.marked.is_none();
        let caret_on = super::caret::blink_visible(
            blink.then_some(self.buffer.cursor),
            self.reveal_cursor,
            window,
            cx,
        );
        let focus = self.focus.clone();
        let move_selection = cx.listener(Self::move_selection);
        let end_selection = cx.listener(Self::end_selection);
        let ui = Ui::new(window, cx);
        ui.input_frame("input", &focus)
            .when(self.number.is_some(), |frame| {
                frame
                    .relative()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_1)
                    .pr(px(2.))
                    .py_0()
            })
            .when(self.search, |field| {
                field
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .pr(px(2.))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .text_color(rgb(ui.colors.muted_foreground))
                            .child(Icon::Search.draw(15.)),
                    )
            })
            .when(self.inline, |field| {
                field.h(rems(1.5)).px(theme::SPACE_1).py_0()
            })
            .when(MULTILINE, |field| {
                field
                    .h(px(90.))
                    .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                        let Some(layout) = &v.layout else { return };
                        let next = (v.scroll.y - e.delta.pixel_delta(layout.height).y)
                            .clamp(px(0.), layout.max_scroll_y());
                        if next != v.scroll.y {
                            v.scroll.y = next;
                            v.reveal_cursor = false;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
            })
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, _, cx| {
                    #[cfg(target_os = "android")]
                    if !v.buffer.read_only {
                        cx.emit(KeyboardRequested);
                    }
                    v.point_cursor(e.position);
                    if !e.modifiers.shift {
                        v.buffer.anchor = v.buffer.cursor;
                    }
                    if e.click_count >= 2 {
                        v.buffer.anchor = 0;
                        v.buffer.cursor = v.buffer.text.len();
                    }
                    v.drag = (!v.buffer.text.is_empty()).then_some(SelectionDrag {
                        pointer: e.position,
                        last_frame: None,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .size_full()
                    .when(self.search || self.number.is_some(), |d| {
                        d.min_w_0().flex_1()
                    })
                    .overflow_hidden()
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, w, cx| {
                                if editable && field.read(cx).is_focused(w) {
                                    w.handle_input(
                                        &focus,
                                        ElementInputHandler::new(bounds, field.clone()),
                                        cx,
                                    );
                                }
                                field.update(cx, |v, cx| v.paint(bounds, caret_on, w, cx));
                                // Keep the drag that began in this input when the pointer leaves its bounds.
                                w.on_mouse_event(move |event, phase, w, cx| {
                                    if phase == DispatchPhase::Capture {
                                        move_selection(event, w, cx);
                                    }
                                });
                                w.on_mouse_event(move |event, phase, w, cx| {
                                    if phase == DispatchPhase::Capture {
                                        end_selection(event, w, cx);
                                    }
                                });
                            },
                        )
                        .size_full(),
                    ),
            )
            .when_some(self.number, |frame, (_, unit)| {
                let frame = frame
                    .on_scroll_wheel(cx.listener(Self::number_wheel))
                    .child(ui.caption(unit).flex_none().whitespace_nowrap())
                    .child(
                        div()
                            .w(px(24.))
                            .h_full()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(rgb(ui.colors.border))
                            .child(self.step_button(true, ui, cx))
                            .child(self.step_button(false, ui, cx)),
                    );
                #[cfg(target_os = "android")]
                let frame = {
                    let field = cx.entity();
                    frame.child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                let field = field.clone();
                                window.on_mouse_event(
                                    move |event: &TouchDragEvent, phase, window, cx| {
                                        if phase == DispatchPhase::Bubble {
                                            field.update(cx, |input, cx| {
                                                input.number_drag(event, bounds, window, cx)
                                            });
                                        }
                                    },
                                );
                            },
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                };
                frame
            })
            .when(self.search, |frame| {
                frame.child(div().flex_none().w(theme::BUTTON_HEIGHT).when(
                    !self.text().is_empty(),
                    |slot| {
                        slot.child(
                            ui.icon_button(Ghost, "clear-search", Icon::Close, "Clear search")
                                .on_click(cx.listener(|field, _, w, cx| {
                                    field.set_text("", cx);
                                    #[cfg(not(target_os = "android"))]
                                    field.focus.focus(w);
                                    #[cfg(target_os = "android")]
                                    field.focus.focus(w, cx);
                                })),
                        )
                    },
                ))
            })
    }
}

impl<const MULTILINE: bool> EntityInputHandler for TextInput<MULTILINE> {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes =
            byte_at_utf16(&self.buffer.text, r.start)..byte_at_utf16(&self.buffer.text, r.end);
        *actual = Some(self.buffer.utf16(bytes.start)..self.buffer.utf16(bytes.end));
        Some(self.buffer.text[bytes].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.buffer.selection();
        Some(UTF16Selection {
            range: self.buffer.utf16(r.start)..self.buffer.utf16(r.end),
            reversed: self.buffer.cursor < self.buffer.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked
            .as_ref()
            .map(|r| self.buffer.utf16(r.start)..self.buffer.utf16(r.end))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_focused(window) && self.buffer.marked.is_some() {
            self.buffer.unmark();
            cx.notify();
        }
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_focused(window) {
            if text.is_empty() && r.is_none() && self.buffer.cancel_composition() {
                self.after_edit();
            } else {
                self.replace(r, text);
            }
            cx.notify();
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_focused(window) {
            if self.number.is_some_and(|(format, _)| !format.permits(text)) {
                return;
            }
            self.buffer.compose(r, text, selection);
            self.after_edit();
            cx.notify();
        }
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let start = byte_at_utf16(&self.buffer.text, r.start);
        let end = byte_at_utf16(&self.buffer.text, r.end);
        let row = layout.row_at(
            start,
            (start == self.buffer.cursor)
                .then_some(self.row_hint)
                .flatten(),
        );
        let x = layout.x(row, start);
        Some(Bounds::new(
            layout.origin(row) + point(x, px(0.)),
            size((layout.x(row, end) - x).max(px(1.)), layout.height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        pt: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.utf16(self.hit(pt).0))
    }
}
