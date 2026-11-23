//! Track the window underneath a window-manager move in desktop coordinates.
use super::*;
use std::{io, os::fd::AsFd};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xinput::{self, ConnectionExt as _, XIEventMask},
        xproto::{
            AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt, EventMask,
            KeyButMask, MapState,
        },
    },
    rust_connection::RustConnection,
};

x11rb::atom_manager! {
    Atoms: AtomsCookie { _NET_CLIENT_LIST, _NET_WM_PID, _NET_WM_NAME, }
}

struct Client {
    frame: u32,
    window: u32,
    title: Vec<u8>,
}

struct Desktop {
    conn: RustConnection,
    root: u32,
    clients: Vec<Client>,
    moving: usize,
}

impl Desktop {
    fn new(title: &str) -> anyhow::Result<Self> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let windows = conn
            .get_property(
                false,
                root,
                atoms._NET_CLIENT_LIST,
                AtomEnum::WINDOW,
                0,
                u32::MAX,
            )?
            .reply()?;
        let mut clients = Vec::new();
        // GPUI 0.2's X11 PlatformWindow does not implement HasWindowHandle.
        // Each workspace has a distinct title within this process.
        for window in windows.value32().into_iter().flatten() {
            let owner = conn
                .get_property(false, window, atoms._NET_WM_PID, AtomEnum::CARDINAL, 0, 1)?
                .reply()?;
            if owner.value32().and_then(|mut p| p.next()) != Some(std::process::id()) {
                continue;
            }
            let title = conn
                .get_property(false, window, atoms._NET_WM_NAME, AtomEnum::ANY, 0, 128)?
                .reply()?
                .value;
            let mut frame = window;
            loop {
                let parent = conn.query_tree(frame)?.reply()?.parent;
                if parent == root || parent == 0 {
                    break;
                }
                frame = parent;
            }
            clients.push(Client {
                frame,
                window,
                title,
            });
        }
        let moving = clients
            .iter()
            .position(|c| c.title == title.as_bytes())
            .ok_or_else(|| anyhow::anyhow!("moving workspace is not mapped"))?;
        Ok(Self {
            conn,
            root,
            clients,
            moving,
        })
    }

    fn beneath(&self, x: i16, y: i16, child: u32) -> anyhow::Result<Option<&Client>> {
        let moving = &self.clients[self.moving];
        if child != moving.frame {
            return Ok(self.clients.iter().find(|c| c.frame == child));
        }
        // Include override-redirect windows and other applications covering a strip.
        for id in self
            .conn
            .query_tree(self.root)?
            .reply()?
            .children
            .into_iter()
            .rev()
        {
            if id == moving.frame {
                continue;
            }
            let Ok(attrs) = self.conn.get_window_attributes(id)?.reply() else {
                continue;
            };
            if attrs.map_state != MapState::VIEWABLE {
                continue;
            }
            let Ok(geom) = self.conn.get_geometry(id)?.reply() else {
                continue;
            };
            let dx = i32::from(x) - i32::from(geom.x);
            let dy = i32::from(y) - i32::from(geom.y);
            if dx >= 0 && dy >= 0 && dx < i32::from(geom.width) && dy < i32::from(geom.height) {
                return Ok(self.clients.iter().find(|c| c.frame == id));
            }
        }
        Ok(None)
    }

    fn target(
        &self,
        v: &Workspace,
        w: &Window,
        cx: &mut App,
        x: i16,
        y: i16,
        child: u32,
    ) -> Option<DockTarget> {
        let client = self.beneath(x, y, child).ok()??;
        let local = self
            .conn
            .translate_coordinates(self.root, client.window, x, y)
            .ok()?
            .reply()
            .ok()?;
        v.dock_target_at(w, cx, |view, target| {
            (view.window_title().as_bytes() == client.title).then(|| {
                point(
                    px(f32::from(local.dst_x) / target.scale_factor()),
                    px(f32::from(local.dst_y) / target.scale_factor()),
                )
            })
        })
    }
}

pub(super) struct Events {
    desktop: Desktop,
    _reader: Task<()>,
}

impl Events {
    pub(super) fn subscribe(title: &str, signal: DragSignal, cx: &App) -> Option<Self> {
        let desktop = Desktop::new(title).ok()?;
        let moving = &desktop.clients[desktop.moving];
        // A separate X connection owns the event selections. A window-manager
        // move grabs the pointer, so also observe XI2 raw release and frame moves.
        let (conn, _) = x11rb::connect(None).ok()?;
        conn.xinput_xi_query_version(2, 1).ok()?.reply().ok()?;
        conn.xinput_xi_select_events(
            desktop.root,
            &[xinput::EventMask {
                deviceid: u16::from(xinput::Device::ALL_MASTER),
                mask: vec![XIEventMask::RAW_MOTION | XIEventMask::RAW_BUTTON_RELEASE],
            }],
        )
        .ok()?
        .check()
        .ok()?;
        conn.xinput_xi_select_events(
            moving.window,
            &[xinput::EventMask {
                deviceid: u16::from(xinput::Device::ALL_MASTER),
                mask: vec![XIEventMask::MOTION | XIEventMask::BUTTON_RELEASE],
            }],
        )
        .ok()?
        .check()
        .ok()?;
        conn.change_window_attributes(
            moving.frame,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )
        .ok()?
        .check()
        .ok()?;
        conn.change_window_attributes(
            moving.window,
            &ChangeWindowAttributesAux::new().event_mask(
                EventMask::STRUCTURE_NOTIFY | EventMask::POINTER_MOTION | EventMask::BUTTON_RELEASE,
            ),
        )
        .ok()?
        .check()
        .ok()?;
        let fd = conn.stream().as_fd().try_clone_to_owned().ok()?;
        let ready = async_io::Async::new_nonblocking(fd).ok()?;
        let reader = cx.background_executor().spawn(async move {
            loop {
                let event = ready
                    .read_with(|_| {
                        conn.poll_for_event()
                            .map_err(io::Error::other)?
                            .ok_or_else(|| io::Error::from(io::ErrorKind::WouldBlock))
                    })
                    .await;
                let phase = match event {
                    Ok(
                        Event::XinputRawMotion(_)
                        | Event::XinputMotion(_)
                        | Event::MotionNotify(_)
                        | Event::ConfigureNotify(_),
                    ) => DragEvent::Moved,
                    Ok(Event::XinputRawButtonRelease(e)) if e.detail == 1 => DragEvent::Released,
                    Ok(Event::XinputButtonRelease(e)) if e.detail == 1 => DragEvent::Released,
                    Ok(Event::ButtonRelease(e)) if e.detail == 1 => DragEvent::Released,
                    Ok(Event::DestroyNotify(_)) | Err(_) => DragEvent::Cancelled,
                    _ => continue,
                };
                signal.send(phase);
                if phase != DragEvent::Moved {
                    break;
                }
            }
        });
        Some(Self {
            desktop,
            _reader: reader,
        })
    }

    pub(super) fn pointer(&self, w: &Window) -> Option<(bool, Point<Pixels>)> {
        let d = &self.desktop;
        let pt = d
            .conn
            .query_pointer(d.clients[d.moving].window)
            .ok()?
            .reply()
            .ok()?;
        Some((
            pt.mask.contains(KeyButMask::BUTTON1),
            point(
                px(f32::from(pt.win_x) / w.scale_factor()),
                px(f32::from(pt.win_y) / w.scale_factor()),
            ),
        ))
    }
}

pub(super) fn tear_off(
    v: &mut Workspace,
    tab: TabDrag,
    placement: Option<DropPlacement>,
    grab: Option<Point<Pixels>>,
    indent: Pixels,
    w: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(index) = v.tabs.iter().position(|p| *p == tab.page) else {
        return;
    };
    if v.tabs.len() < 2 {
        return;
    }
    v.take_tab(&tab.page, w, cx);
    let source = cx.weak_entity();
    // The callback runs before painting. Queue native window creation on the
    // UI executor so the source can submit its remaining page in this frame.
    w.on_next_frame(move |w, cx| {
        w.spawn(cx, async move |cx| {
            let _ = source.update_in(cx, |v, w, cx| {
                if tab.page.shell_exited(cx)
                    || v.tabs.contains(&tab.page)
                    || v.other_window_with(w, cx, |p| *p == tab.page).is_some()
                {
                    return;
                }
                if let Some(dest) = v.open_tab_window(tab.page.clone(), placement, indent, cx) {
                    if let Some(grab) = grab {
                        let _ = dest.update(cx, |_, w, cx| start(cx.weak_entity(), w, tab, grab));
                    }
                } else {
                    v.tabs.insert(index.min(v.tabs.len()), tab.page);
                    v.refresh_title(w);
                    v.focus_tab(tab.previous_active, w, cx);
                }
            });
        })
        .detach();
    });
}

pub(super) fn start(
    view: WeakEntity<Workspace>,
    window: &Window,
    tab: TabDrag,
    grab: Point<Pixels>,
) {
    window.on_next_frame(move |window, cx| {
        let _ = view.update(cx, |v, cx| {
            let mut previous = None;
            let mut held = true;
            let title = v.window_title();
            let grab = tab.window_grab(v.dock.read(cx)).unwrap_or(grab);
            v.watch_drag(
                window,
                cx,
                |signal, w, cx| {
                    let events = Events::subscribe(&title, signal, cx)?;
                    let d = &events.desktop;
                    let pt = d.conn.query_pointer(d.root).ok()?.reply().ok()?;
                    held = pt.mask.contains(KeyButMask::BUTTON1);
                    if held {
                        let offset = grab.scale(w.scale_factor());
                        let xy = ConfigureWindowAux::new()
                            .x(i32::from(pt.root_x) - f64::from(offset.x).round() as i32)
                            .y(i32::from(pt.root_y) - f64::from(offset.y).round() as i32);
                        let _ = d
                            .conn
                            .configure_window(d.clients[d.moving].window, &xy)
                            .ok()?
                            .check();
                    }
                    Some(events)
                },
                move |events, event, v, w, cx| {
                    if event == DragEvent::Cancelled {
                        return false;
                    }
                    let d = &events.desktop;
                    let Some(pt) = d
                        .conn
                        .query_pointer(d.root)
                        .ok()
                        .and_then(|c| c.reply().ok())
                    else {
                        return false;
                    };
                    let pressed =
                        event != DragEvent::Released && pt.mask.contains(KeyButMask::BUTTON1);
                    let position = (pt.root_x, pt.root_y, pt.child);
                    if (!pressed || previous != Some(position))
                        && let Some(target) = d.target(v, w, cx, pt.root_x, pt.root_y, pt.child)
                        && v.dock_at_pointer(target, pressed.then_some(tab.offset), w, cx)
                    {
                        return false;
                    }
                    previous = Some(position);
                    pressed
                },
            );
            // An unavailable observer should still allow the native window move.
            if held {
                window.start_window_move();
            } else {
                v.settle_tab(cx);
            }
        });
    });
}
