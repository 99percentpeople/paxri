use paxri_ui::terminal::bell::BellState;
use paxri_ui::terminal::cursor;
use paxri_ui::terminal::display;
use paxri_ui::terminal::emulator;
use paxri_ui::terminal::keys;
use paxri_ui::terminal::lifecycle::{
    LifecycleEffect, LifecycleEvent, PeerConnectionState, PeerLifecycle, SessionAction,
    SessionState, StreamEnd,
};
use paxri_ui::terminal::links::TerminalLink;
mod mouse;
mod session;
mod touch;
pub(crate) use paxri_ui::terminal::title::TabTitle;

use crate::components::{ScrollMetrics, ScrollSource, Scrollbar, Ui};
use crate::theme::{CONTROL_RADIUS, SPACE_2, TextRole, TypographyExt, Zoom};
use crate::theme::fonts::{CellSize, MonospaceFonts};
use alacritty_terminal::{grid::Dimensions, grid::Scroll, term::TermMode};
use emulator::{Emulator, GridSize};
use gpui::{prelude::*, *};
use session::{Output, Session, SessionEvent};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

/// Shared by terminal views in this process. Window chrome uses the platform DPI.
struct TerminalZoom(f32);
impl Global for TerminalZoom {}

impl TerminalZoom {
    fn current(cx: &App) -> f32 {
        cx.try_global::<Self>().map_or(1., |zoom| zoom.0)
    }

    fn apply(zoom: Zoom, window: &mut Window, cx: &mut App) {
        let current = Self::current(cx);
        let scale = zoom.scale(current);
        if current != scale {
            cx.set_global(Self(scale));
        }
        window.prevent_default();
        cx.stop_propagation();
    }
}

fn layout_grid(available: Bounds<Pixels>, cell: CellSize) -> (GridSize, Bounds<Pixels>) {
    let grid = GridSize::from_viewport(available.size, cell);
    let cells = size(
        px(f32::from(grid.cols) * cell.width).min(available.size.width),
        px(f32::from(grid.rows) * cell.height).min(available.size.height),
    );
    (grid, Bounds::centered_at(available.center(), cells))
}

fn play_system_bell(cx: &App) {
    #[cfg(target_os = "macos")]
    {
        let _ = cx;
        objc2_app_kit::NSBeep();
    }
    #[cfg(windows)]
    {
        cx.background_executor()
            .spawn(async move {
                unsafe {
                    windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
                        windows_sys::Win32::UI::WindowsAndMessaging::MB_OK,
                    );
                }
            })
            .detach();
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cx.background_executor()
            .spawn(async move {
                let played = std::process::Command::new("canberra-gtk-play")
                    .args(["-i", "bell"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !played
                    && let Ok((conn, _)) = x11rb::connect(None)
                {
                    use x11rb::connection::Connection as _;
                    let _ = x11rb::protocol::xproto::bell(&conn, 0);
                    let _ = conn.flush();
                }
            })
            .detach();
    }
}

pub struct Terminal {
    focus: FocusHandle,
    pub(super) title: Entity<TabTitle>,
    pub(super) bell: Entity<BellState>,
    peer: String,
    key: paxri_control::terminal::managed::Key,
    ipc: Option<PathBuf>,
    runtime: tokio::runtime::Handle,
    emulator: Emulator,
    lifecycle: PeerLifecycle,
    session: Option<Session>,
    error: Option<String>,
    marked: cursor::Preedit,
    viewport: Bounds<Pixels>,
    selecting: bool,
    mouse: mouse::Mouse,
    touch: touch::Touch,
    link_modifier: bool,
    hovered_link: Option<TerminalLink>,
    pending_link: Option<TerminalLink>,
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

#[derive(Clone)]
struct TerminalHistory(Entity<Terminal>);

impl ScrollSource for TerminalHistory {
    fn metrics(&self, cx: &App) -> ScrollMetrics {
        self.0.read(cx).emulator.scroll_metrics()
    }

    fn scroll_to(&self, offset: f32, cx: &mut App) {
        self.0.update(cx, |view, cx| {
            if view.emulator.scroll_to_offset(offset) {
                cx.notify();
            }
        });
    }
}

const LOCAL_SESSION_ID: u64 = 0;

impl Terminal {
    pub(super) fn open(
        peer: &str,
        key: paxri_control::terminal::managed::Key,
        attach: bool,
        connection_id: u64,
        runtime: &tokio::runtime::Handle,
        ipc: Option<&Path>,
        window: &Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let appearance = crate::theme::current(cx).terminal;
        let font = cx.global::<MonospaceFonts>().resolve(&appearance).scaled(TerminalZoom::current(cx));
        let grid = GridSize::from_viewport(window.viewport_size(), font.cell);
        let palette =
            crate::theme::terminal_palette(appearance.scheme, crate::theme::palette(window, cx));
        let peer = peer.to_owned();
        let ipc = ipc.map(Path::to_owned);
        let runtime = runtime.clone();
        cx.new(|cx| {
            cx.observe_global::<TerminalZoom>(|_, cx| cx.notify()).detach();
            let focus = cx.focus_handle();
            let mut emulator = Emulator::with_palette(grid, palette, font);
            emulator.set_default_cursor(cursor::default_style(&appearance));
            let mut terminal = Terminal {
                focus,
                title: cx.new(|_| TabTitle::default()),
                bell: cx.new(|_| BellState::default()),
                peer,
                key,
                ipc,
                runtime,
                emulator,
                lifecycle: PeerLifecycle::default(),
                session: None,
                error: None,
                marked: cursor::Preedit::default(),
                viewport: Bounds::default(),
                selecting: false,
                touch: Default::default(),
                mouse: mouse::Mouse::default(),
                link_modifier: false,
                hovered_link: None,
                pending_link: None,
                hidden_pointer_at: None,
                focused: false,
                focus_observers: None,
            };
            terminal.apply_lifecycle(
                LifecycleEvent::ConnectionChanged(PeerConnectionState::Connected { connection_id }),
                cx,
            );
            terminal.apply_lifecycle(
                LifecycleEvent::OpenSession {
                    id: LOCAL_SESSION_ID,
                    attach,
                },
                cx,
            );
            terminal
        })
    }

    fn grid_size(&self) -> GridSize {
        GridSize {
            rows: self.emulator.term.screen_lines() as u16,
            cols: self.emulator.term.columns() as u16,
        }
    }

    fn spawn_stream(&mut self, attach: bool, generation: u64, cx: &mut Context<Self>) {
        let (mut session, events) = Session::start(
            &self.peer,
            self.key,
            attach,
            self.ipc.as_deref(),
            &self.runtime,
        );
        if self.viewport.size.width > px(0.) && self.viewport.size.height > px(0.) {
            session.resize(self.grid_size().pty(self.emulator.font.cell));
        }
        self.session = Some(session);
        self.error = None;
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let Ok(pending) = this.update(cx, |view: &mut Terminal, cx| {
                    if view.generation() != generation {
                        return None;
                    }
                    view.output(generation, event, cx);
                    // Coalesce ready chunks into the same frame.
                    for event in std::iter::from_fn(|| events.try_recv().ok()).take(32) {
                        if view.generation() != generation {
                            break;
                        }
                        view.output(generation, event, cx);
                    }
                    let reply = view.emulator.finish_batch_with_clipboard(|_target, text| {
                        if view.focused && crate::theme::terminal_controls(cx).remote_clipboard {
                            let item = ClipboardItem::new_string(text);
                            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                            if _target == alacritty_terminal::term::ClipboardType::Selection {
                                cx.write_to_primary(item);
                                return;
                            }
                            cx.write_to_clipboard(item);
                        }
                    });
                    if let Some(title) = view.emulator.take_title_update() {
                        view.title.update(cx, |tab, cx| {
                            if tab.set_pty(title) {
                                cx.notify();
                            }
                        });
                    }
                    if view.emulator.take_bell()
                        && crate::theme::terminal_controls(cx).terminal_bell
                    {
                        let play = view.bell.update(cx, |bell, cx| bell.ring(cx));
                        if play {
                            play_system_bell(cx);
                        }
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
    }

    fn spawn_end(&mut self, generation: u64, cx: &mut Context<Self>) {
        let address = self
            .ipc
            .clone()
            .unwrap_or_else(paxri_control::default_address);
        let peer = self.peer.clone();
        let key = self.key;
        let job = self.runtime.spawn(async move {
            paxri_control::managed_terminal(
                &address,
                peer,
                paxri_control::terminal::managed::Request::End(key),
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let end = match job.await {
                Ok(Ok(_)) => StreamEnd::Exited(0),
                Ok(Err(e)) if e.is::<paxri_control::terminal::managed::Rejected>() => {
                    StreamEnd::Rejected(format!("{e:#}"))
                }
                Ok(Err(e)) if e.is::<tokio::time::error::Elapsed>() => StreamEnd::TimedOut,
                Ok(Err(e)) => StreamEnd::Disconnected(format!("{e:#}")),
                Err(e) => StreamEnd::Disconnected(e.to_string()),
            };
            let _ = this.update(cx, |view, cx| {
                view.apply_lifecycle(
                    LifecycleEvent::StreamEnded {
                        id: LOCAL_SESSION_ID,
                        generation,
                        end,
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    fn session_state(&self) -> Option<&SessionState> {
        self.lifecycle.session_state(LOCAL_SESSION_ID)
    }

    fn generation(&self) -> u64 {
        self.lifecycle
            .session(LOCAL_SESSION_ID)
            .map_or(0, |s| s.generation)
    }

    pub(super) fn on_connection_changed(
        &mut self,
        state: PeerConnectionState,
        cx: &mut Context<Self>,
    ) {
        self.apply_lifecycle(LifecycleEvent::ConnectionChanged(state), cx);
    }

    fn apply_lifecycle(&mut self, event: LifecycleEvent, cx: &mut Context<Self>) {
        let effects = self.lifecycle.handle(event);
        let mut dispatch_pending = false;
        for effect in effects {
            match effect {
                LifecycleEffect::SpawnCreate { generation, .. } => {
                    self.spawn_stream(false, generation, cx);
                }
                LifecycleEffect::SpawnAttach { generation, .. } => {
                    self.spawn_stream(true, generation, cx);
                }
                LifecycleEffect::SpawnEnd { generation, .. } => {
                    self.spawn_end(generation, cx);
                }
                LifecycleEffect::AbortJob { .. } => {
                    self.session = None;
                    if let Some(SessionState::Ended(reason)) = self.session_state() {
                        self.error = Some(reason.clone());
                    }
                }
                LifecycleEffect::SendExit { .. } => {
                    if let Some(session) = &mut self.session {
                        session.end();
                    }
                }
                LifecycleEffect::DismissSession { .. } => {
                    self.session = None;
                    self.error = None;
                    cx.emit(ShellExited);
                }
                LifecycleEffect::StopInput { .. } => {
                    self.marked.clear();
                    if let Some(SessionState::Ended(reason)) = self.session_state() {
                        self.error = Some(reason.clone());
                    }
                }
                LifecycleEffect::ClearComposing { .. } => {
                    self.marked.clear();
                }
                LifecycleEffect::ScheduleRetry { id } => {
                    let timer = self
                        .runtime
                        .spawn(async { tokio::time::sleep(std::time::Duration::from_secs(1)).await });
                    cx.spawn(async move |this, cx| {
                        let _ = timer.await;
                        let _ = this.update(cx, |view, cx| {
                            view.apply_lifecycle(LifecycleEvent::RetryTimerElapsed { id }, cx);
                        });
                    })
                    .detach();
                }
                LifecycleEffect::DispatchPending => {
                    dispatch_pending = true;
                }
                _ => {}
            }
        }
        if dispatch_pending {
            self.apply_lifecycle(
                LifecycleEvent::DispatchPending {
                    active_session: Some(LOCAL_SESSION_ID),
                    visible: true,
                },
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn is_running(&self) -> bool {
        self.session_state().is_some_and(SessionState::is_open)
    }

    pub(super) fn is_confirmed_on_host(&self) -> bool {
        self.session_state()
            .is_some_and(SessionState::is_confirmed_on_host)
    }

    pub(super) fn action(&self) -> SessionAction {
        self.session_state()
            .map_or(SessionAction::Close, SessionState::action)
    }

    pub(super) fn end(&mut self, cx: &mut Context<Self>) {
        self.apply_lifecycle(LifecycleEvent::UserStop { id: LOCAL_SESSION_ID }, cx);
    }

    pub(super) fn has_exited(&self) -> bool {
        self.session_state().is_none_or(SessionState::is_ended) && self.error.is_none()
    }

    pub(super) fn is_bell_ringing(&self, cx: &App) -> bool {
        self.bell.read(cx).is_ringing()
    }

    fn status_banner(&self) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(error.clone());
        }
        match self.session_state()? {
            SessionState::Initializing => Some("Starting session…".into()),
            SessionState::Attaching => Some("Connecting…".into()),
            SessionState::Reconnecting => match self.lifecycle.connection() {
                PeerConnectionState::Connected { .. } => Some("Reconnecting…".into()),
                PeerConnectionState::Initializing | PeerConnectionState::Reconnecting => {
                    Some("Reconnecting device…".into())
                }
                PeerConnectionState::Stopping => Some("Connection stopping…".into()),
                PeerConnectionState::Disconnected => Some("Device offline".into()),
            },
            SessionState::Stopping => Some("Stopping session…".into()),
            SessionState::Active | SessionState::Ended(_) => None,
        }
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
    fn output(&mut self, generation: u64, event: SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::HostConfirmed => {
                self.apply_lifecycle(
                    LifecycleEvent::HostConfirmed {
                        id: LOCAL_SESSION_ID,
                        generation,
                    },
                    cx,
                );
            }
            SessionEvent::Connected => {
                self.apply_lifecycle(
                    LifecycleEvent::SessionConnected {
                        id: LOCAL_SESSION_ID,
                        generation,
                    },
                    cx,
                );
            }
            SessionEvent::Output(Output::Replay(bytes)) => {
                self.apply_lifecycle(
                    LifecycleEvent::HostConfirmed {
                        id: LOCAL_SESSION_ID,
                        generation,
                    },
                    cx,
                );
                let appearance = crate::theme::current(cx).terminal;
                let mut fresh = Emulator::with_palette(
                    self.grid_size(),
                    self.emulator.palette,
                    self.emulator.font.clone(),
                );
                fresh.set_default_cursor(cursor::default_style(&appearance));
                self.emulator = fresh;
                self.emulator.process(&bytes);
                let _ = self.emulator.finish_batch();
                let _ = self.emulator.take_bell();
            }
            SessionEvent::Output(Output::Bytes(bytes)) => {
                self.apply_lifecycle(
                    LifecycleEvent::SessionConnected {
                        id: LOCAL_SESSION_ID,
                        generation,
                    },
                    cx,
                );
                self.emulator.process(&bytes);
            }
            SessionEvent::Output(Output::Error(error)) => {
                if self.is_running() {
                    self.error = Some(format!("Terminal I/O: {error}"));
                }
            }
            SessionEvent::Output(Output::Exited(_)) => {}
            SessionEvent::Ended(end) => {
                self.session = None;
                let exited_cleanly = matches!(end, StreamEnd::Exited(_));
                self.apply_lifecycle(
                    LifecycleEvent::StreamEnded {
                        id: LOCAL_SESSION_ID,
                        generation,
                        end,
                    },
                    cx,
                );
                if exited_cleanly && self.session_state().is_some_and(SessionState::is_ended) {
                    self.error = None;
                    cx.emit(ShellExited);
                }
            }
        }
    }
    fn cell_at(&self, position: Point<Pixels>) -> (usize, f32) {
        let local = position - self.viewport.origin;
        self.emulator
            .grid_position(f32::from(local.x), f32::from(local.y))
    }

    pub(super) fn link_modifier_active(modifiers: Modifiers) -> bool {
        modifiers == Modifiers::secondary_key()
    }

    pub(super) fn link_at(&self, position: Point<Pixels>, cx: &App) -> Option<TerminalLink> {
        if !crate::theme::terminal_controls(cx).clickable_links || !self.viewport.contains(&position)
        {
            return None;
        }
        let (row, x) = self.cell_at(position);
        self.emulator.link_at(row, x)
    }

    pub(super) fn update_hovered_link(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let active = Self::link_modifier_active(modifiers);
        self.link_modifier = active;
        let next = (active
            && !self.selecting
            && self.mouse.held == 0
            && self.hidden_pointer_at.is_none())
        .then(|| self.link_at(position, cx))
        .flatten();
        if next != self.hovered_link {
            self.hovered_link = next;
            cx.notify();
        }
    }

    fn start_selection(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(w, cx);
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
                self.link_modifier = false;
                self.hovered_link = None;
                self.pending_link = None;
                self.hidden_pointer_at = None;
                self.cancel_touch();
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
        if self
            .session_state()
            .is_some_and(|s| s.is_open() && !s.is_stopping())
            && !bytes.is_empty()
        {
            self.hidden_pointer_at = self.typing_pointer_position(window);
            self.cancel_touch();
            self.touch.handles = false;
            self.emulator.input_started();
            self.emulator.term.selection = None;
            self.selecting = false;
            self.emulator.scroll(Scroll::Bottom);
            self.send(bytes);
        }
    }
    fn paste(&mut self, window: &Window, cx: &App) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let bytes = self.emulator.paste_bytes(&text);
            self.write(bytes, window);
        }
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let controls = crate::theme::terminal_controls(cx);
        let action = keys::local_action(key, controls);
        match keys::terminal_input(key, action, &self.emulator) {
            Some(keys::TerminalInput::Copy(text)) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Some(keys::TerminalInput::Paste) => self.paste(window, cx),
            Some(keys::TerminalInput::Zoom(zoom)) => return TerminalZoom::apply(zoom, window, cx),
            Some(keys::TerminalInput::Send(bytes)) => self.write(bytes, window),
            None => return,
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
        self.emulator.font = cx
            .global::<MonospaceFonts>()
            .resolve(&appearance)
            .scaled(TerminalZoom::current(cx));
        let background = self.emulator.sync_appearance(ui.colors, &appearance);
        let palette = self.emulator.palette;
        self.observe_focus(window, cx);
        if self.hidden_pointer_at.is_some()
            && self.hidden_pointer_at != self.typing_pointer_position(window)
        {
            self.hidden_pointer_at = None;
        }
        let hide_pointer = self.hidden_pointer_at.is_some();
        let pointer = window.mouse_position();
        let hovered_link = (self.link_modifier
            && window.is_window_hovered()
            && !self.selecting
            && self.mouse.held == 0
            && !hide_pointer)
        .then(|| self.link_at(pointer, cx))
        .flatten();
        self.hovered_link = hovered_link.clone();
        let preview_top = hovered_link.is_some() && pointer.y > self.viewport.bottom() - px(48.);
        let cursor_shape = self.emulator.cursor_for_paint(
            self.focused && self.is_running(),
            !self.marked.text.is_empty(),
            window,
            cx,
        );
        let entity = cx.entity();
        let sizing = entity.clone();
        let focus = self.focus.clone();
        let screen = canvas(
            move |bounds, w, cx| {
                let grid = sizing.update(cx, |view, cx| view.resize(bounds, cx));
                (grid, w.insert_hitbox(grid, HitboxBehavior::Normal))
            },
            move |_, (bounds, hitbox), window, cx| {
                Self::listen_touch(&entity, hitbox, window);
                if hide_pointer {
                    window.hide_cursor_until_mouse_moves(cx);
                }
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
                entity.update(cx, |view, cx| {
                    display::paint(
                        &view.emulator,
                        bounds,
                        (!view.marked.text.is_empty()).then_some(&view.marked),
                        cursor_shape,
                        view.hovered_link.as_ref(),
                        palette,
                        window,
                        cx,
                    );
                    view.paint_grips(window);
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
            .on_modifiers_changed(cx.listener(|view, e: &ModifiersChangedEvent, window, cx| {
                view.update_hovered_link(window.mouse_position(), e.modifiers, cx);
            }))
            .when_some(self.status_banner(), |d, message| {
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
                        .child(div().flex_1().text_role(TextRole::Body).child(message)),
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
                    .cursor(if hovered_link.is_some() {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .on_any_mouse_down(cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_hover))
                    .on_pinch(|event, window, cx| TerminalZoom::apply(Zoom::Pinch(1. + event.delta), window, cx))
                    .on_scroll_wheel(cx.listener(Self::wheel))
                    .child(screen)
                    .child(Scrollbar::vertical("terminal-history", TerminalHistory(cx.entity())))
                    .when_some(hovered_link, |screen, link| {
                        let pill = div()
                            .absolute()
                            .left_2()
                            .when(preview_top, |d| d.top_2())
                            .when(!preview_top, |d| d.bottom_2())
                            .max_w(px(460.))
                            .px_2()
                            .py_1()
                            .rounded(CONTROL_RADIUS)
                            .border_1()
                            .border_color(rgb(ui.colors.border))
                            .bg(rgb(ui.colors.card))
                            .text_color(rgb(ui.colors.canvas.foreground))
                            .text_role(TextRole::Caption)
                            .truncate()
                            .shadow_sm()
                            .child(link.uri);
                        screen.child(pill)
                    })
                    .when(self.touch.handles && self.emulator.selected_range().is_some(), |screen| screen.child(
                        div().absolute().right_3().top_2().occlude().child(
                            ui.button(crate::components::ButtonVariant::Outline, "copy-touch-selection", "Copy")
                                .on_click(cx.listener(|v, _, w, cx| {
                                    if let Some(text) = v.emulator.selected_text() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    }
                                    v.touch.handles = false;
                                    v.emulator.term.selection = None;
                                    v.focus.focus(w, cx);
                                    cx.notify();
                                }))
                        )
                    )),
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
