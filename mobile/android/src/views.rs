use super::*;
use paxri_ui::components::{ListState, ListView};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use paxri_core::connection::{peer_state, unix_time_ms};
use paxri_ui::connections::presence_detail;
use paxri_ui::{
    components::{
        ButtonVariant::Ghost,
        Icon, single_line, floating_corner, FLOATING_ACTION_CLEARANCE,
    },
    terminal::{cursor, display},
};

fn app_bar_height(window: &Window) -> f32 {
    // Keep room for both title lines when the system accessibility text size increases.
    (2.75 * f32::from(window.rem_size()) + 8.).max(56.)
}


fn keybar_height(window: &Window, keys: &KeybarSettings) -> f32 {
    let [_, top, _, bottom] = *EDGES.lock().unwrap();
    let available = f32::from(window.viewport_size().height) - top - bottom - app_bar_height(window);
    (keys.rows.len() as f32 * f32::from(keys.row_height)).min((available * 0.45).max(0.))
}

pub(super) fn initial_size(window: &Window, cell: CellSize, keys: &KeybarSettings) -> GridSize {
    let [left, top, right, bottom] = *EDGES.lock().unwrap();
    let available = window.viewport_size();
    GridSize::from_viewport(
        size(
            available.width - px(left + right),
            available.height - px(top + bottom + app_bar_height(window) + keybar_height(window, keys)),
        ),
        cell,
    )
}

impl Mobile {
    fn app_bar(&self, ui: Ui, window: &Window, cx: &Context<Self>) -> Div {
        let (title, subtitle, back, end) = match &self.page {
            Page::Devices | Page::PairingMenu => {
                let total = self.peers.len();
                let connected = self
                    .peers
                    .keys()
                    .filter(|id| self.presence.online(id))
                    .count();
                let subtitle = (self.client.is_some() && total > 0)
                    .then(|| format!("{connected} of {total} connected"));
                (SharedString::from("Devices"), subtitle, None, None)
            }
            Page::Sessions(origin) => (
                "Sessions".into(), None, origin.as_ref().map(|_| "Back to device details"), None,
            ),
            Page::Peer(id) => (
                self.peers[id].name.clone().into(),
                None,
                Some("Back to devices"),
                Some(
                    ui.connection_badge(self.presence.paths_for(id), true)
                        .into_any_element(),
                ),
            ),
            Page::ForwardEdit { owner, .. } => (
                (if self.rule_form.existing.is_some() {
                    "Edit forwarding rule"
                } else {
                    "New forwarding rule"
                })
                .into(),
                None,
                Some("Back to forwarding rules"),
                self.save_rule_button(owner.as_deref(), ui, cx),
            ),
            Page::Forwards(peer) => (
                "Port forwarding".into(),
                None,
                peer.as_ref().map(|_| "Back to device details"),
                None,
            ),
            Page::Settings => (SharedString::from("Settings"), None, None, None),
            Page::SettingsCategory(section) => (
                SharedString::from(section.title),
                section.table.heading_label().map(|label| label.to_string()),
                Some("Back to settings"),
                None,
            ),
            Page::Terminal(id) => {
                let id = *id;
                let session = &self.sessions[&id];
                let (title, subtitle) = session.labels(cx);
                let action = match session.state {
                    SessionState::Connecting | SessionState::Connected => Some(ui.end_session_button()),
                    SessionState::Stopping => None,
                    SessionState::Ended(_) => Some(
                        ui.button(Ghost, "close-session", Icon::Close.draw(20.))
                            .circular()
                            .tooltip(paxri_ui::components::tooltip("Close session".into()))
                            .role(Role::Button)
                            .aria_label("Close session"),
                    ),
                }
                .map(|button| button
                    .on_click(cx.listener(move |this, _, _, cx| this.end_session(id, cx)))
                    .into_any_element());
                (
                    title,
                    Some(subtitle),
                    Some(if matches!(self.terminal_return, Page::Sessions(_)) { "Back to sessions" } else { "Back to device details" }),
                    action,
                )
            }
        };
        div()
            .w_full()
            .h(px(app_bar_height(window)))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(4.))
            .pr_3()
            .border_b_1()
            .border_color(rgb(ui.colors.border))
            .bg(rgb(ui.colors.card))
            .child(match back {
                Some(label) => ui
                    .button(Ghost, "back", Icon::ArrowLeft.draw(22.))
                    .circular()
                    .role(Role::Button)
                    .aria_label(label)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.back(cx);
                        cx.notify();
                    })),
                None => ui
                    .button(Ghost, "open-navigation", Icon::Menu.draw(22.))
                    .circular()
                    .role(Role::Button)
                    .aria_label("Open navigation menu")
                    .aria_expanded(self.drawer.open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.drawer.set_open(true);
                        this.sync_back(cx);
                        cx.notify();
                    })),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when_some(subtitle, |labels, subtitle| {
                        labels.child(ui.muted(subtitle).text_xs().truncate())
                    }),
            )
            .children(end)
    }

    fn pairing_actions(&self, expanded: bool, ui: Ui, cx: &Context<Self>) -> Div {
        let action = |id: &'static str, label: &'static str, icon, action| {
            let text = div().h(px(48.)).flex().items_center().child(label);
            ui.button(Ghost, id, text)
                .row()
                .icon(icon)
                .role(Role::Button)
                .aria_label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.navigate(Page::Devices, cx);
                    if let Err(error) = activity_action(&this.android, action) {
                        Toast::show(error, this.window, cx);
                    }
                    cx.notify();
                }))
        };
        let toggle_icon = if expanded { Icon::Close } else { Icon::Plus };
        floating_corner()
            .flex()
            .flex_col()
            .items_end()
            .gap_3()
            .when(expanded, |stack| {
                stack.child(
                    div()
                        .occlude()
                        .w(px(192.))
                        .p_1()
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(ui.colors.border))
                        .bg(rgb(ui.colors.card))
                        .shadow_md()
                        .flex()
                        .flex_col()
                        .child(action(
                            "enter-code",
                            "Enter pairing code",
                            Icon::Pencil,
                            ActivityAction::Pair,
                        ))
                        .child(action(
                            "scan-qr",
                            "Scan QR code",
                            Icon::ScanQr,
                            ActivityAction::Scan,
                        )),
                )
            })
            .child(
                ui.floating_action(
                    "pair-device",
                    toggle_icon,
                    if expanded {
                        "Close pairing menu"
                    } else {
                        "Pair device"
                    },
                )
                .aria_expanded(expanded)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.navigate(
                        if expanded {
                            Page::Devices
                        } else {
                            Page::PairingMenu
                        },
                        cx,
                    );
                    cx.notify();
                })),
            )
    }

    fn devices(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let now = unix_time_ms();
        let filters = &self.device_filters;
        let query = filters.query(cx);
        let mut peers: Vec<_> = self
            .peers
            .iter()
            .filter(|(id, peer)| filters.matches(&query, &peer.name, "", id, peer.platform, peer_state(self.presence.paths_for(id), true)))
            .map(|(id, peer)| {
                let changed = self
                    .presence
                    .changed_at
                    .get(id)
                    .copied()
                    .max(peer.last_status_change_ms);
                (id, peer, changed)
            })
            .collect();
        peers.sort_by_key(|(_, peer, changed)| (std::cmp::Reverse(*changed), &peer.name));
        let no_matches = peers.is_empty() && !self.peers.is_empty();
        let rows = peers.into_iter().map(|(id, peer, changed)| {
            let id = id.clone();
            ui.device_row(
                id.clone(),
                peer.platform,
                peer.name.clone().into(),
                self.presence.online(&id),
                presence_detail(peer_state(self.presence.paths_for(&id), true), now, changed),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.navigate(Page::Peer(id.clone()), cx);
                cx.notify();
            }))
        });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_3()
            .pt_2()
            .gap_2()
            .when(!self.peers.is_empty(), |page| page.child(filters.controls(|v| &mut v.device_filters, cx).with_state(&self.filter_menu)))
            .when(no_matches, |page| page.child(ListState::filtered("No matching devices").render(ui)))
            .when(self.client.is_none(), |page| {
                page.child(if self.starting_network { ListState::loading("Starting network…") } else { ListState::empty(Icon::Server, "Network unavailable") }.render(ui))
            })
            .when(self.pairing, |page| page.child(ui.muted("Approve the request on your computer.")))
            .when(
                self.peers.is_empty() && self.client.is_some() && !self.pairing,
                |page| page.child(ListState::empty(Icon::Monitor, "No paired devices").detail("Tap + to pair a computer.").render(ui)),
            )
            .when(!no_matches && !self.peers.is_empty(), |page| page.child(
                ListView::scrollable("paired-devices")
                    .with_floating_action(self.client.is_some() && !self.pairing)
                    .children(rows),
            ))
    }

    fn peer(&self, id: &str, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = &self.peers[id];
        let paths = self.presence.paths_for(id);
        let can_connect = self.presence.online(id) && peer.supports_shell();
        let body = div()
            .id("device-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .px(px(18.))
            .flex()
            .flex_col()
            .gap_4()
            .pt_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(ui.device_icon(peer.platform))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(peer.platform.label()),
                            )
                            .child(
                                single_line(ui.muted(format!("Device ID · {}", &id[..12])))
                                    .text_xs()
                                    .id("device-identity")
                                    .tooltip(paxri_ui::components::tooltip(
                                        id.to_owned().into(),
                                    )),
                            ),
                    ),
            )
            .when_some(paths.filter(|p| !p.is_empty()), |page, paths| {
                page.child(ui.separator())
                    .child(self.connection_history.connection_paths(id, paths, ui))
            })
            .when(peer.supports_forward_rules(), |page| {
                let id = id.to_owned();
                page.child(ui.separator()).child(
                    ui.navigation_row(
                        "device-forwards",
                        Icon::Forward.draw(18.),
                        "Port forwarding".into(),
                        "View forwarding rules",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate(Page::Forwards(Some(id.clone())), cx);
                        cx.notify();
                    })),
                )
            })
            .when(can_connect, |page| page.pb(FLOATING_ACTION_CLEARANCE))
            .child(ui.separator())
            .child(ui.navigation_row("device-sessions", Icon::Terminal.draw(18.), "Sessions".into(), "View sessions on this device")
                .on_click(cx.listener({
                    let id = id.to_owned();
                    move |this, _, _, cx| this.show_sessions(Some(id.clone()), cx)
                })));
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(body)
            .when(can_connect, |page| {
                let target = id.to_owned();
                let button = ui.new_session_button()
                    .on_click(cx.listener(move |this, _, w, cx| this.open(target.clone(), None, w, cx)));
                page.child(floating_corner().child(button))
            })
    }

    fn terminal(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let toolbar = keybar_height(window, &self.keybar);
        let s = self.sessions.get_mut(&id).expect("active terminal session");
        let appearance = theme::current(cx).terminal;
        s.screen
            .set_default_cursor(cursor::default_style(&appearance));
        s.screen.palette = theme::terminal_palette(appearance.scheme, Ui::new(window, cx).colors);
        let view = cx.entity();
        let sizing = view.clone();
        let background = display::color(
            Color::Named(NamedColor::Background),
            s.screen.palette,
            s.screen.term.colors(),
        );
        let style = s.screen.display_cursor_style();
        let blink = style.blinking && s.connected() && s.composing.text.is_empty();
        let show_cursor =
            cursor::blink_visible(blink, cursor::CursorCell::visible(&s.screen), window, cx);
        let screen = canvas(
            move |bounds, window, cx| {
                let scale = window.scale_factor();
                let area = [
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.size.width,
                    bounds.size.height,
                    px(toolbar),
                ]
                .map(|v| (f32::from(v) * scale).round() as i32);
                sizing.update(cx, |this, _| {
                    this.set_touch_area(id, area, this.keybar.clone());
                    if let Some(s) = this.sessions.get_mut(&id) {
                        let grid = GridSize::from_viewport(bounds.size, s.screen.font.cell);
                        if s.screen.resize(grid) {
                            let _ = s.input.send(Frame::Resize(PtySize {
                                rows: grid.rows,
                                cols: grid.cols,
                            }));
                        }
                    }
                })
            },
            move |bounds, _, window, cx| {
                view.update(cx, |this, cx| {
                    if let Some(s) = this.sessions.get(&id) {
                        let marked = (!s.composing.text.is_empty()).then_some(&s.composing);
                        display::paint(
                            &s.screen,
                            bounds,
                            marked,
                            show_cursor.then_some(style.shape),
                            s.screen.palette,
                            window,
                            cx,
                        );
                    }
                })
            },
        )
        .size_full();
        let grid = div()
            .id("terminal")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(background))
            .child(screen);
        // The Android touch surface owns the multi-pointer key row beneath the canvas.
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(grid)
            .child(div().h(px(toolbar)).flex_none())
    }
}

impl Render for Mobile {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(*SYSTEM_UI_REM.lock().unwrap()));
        let [left, top, right, bottom] = *EDGES.lock().unwrap();
        if bottom > self.bottom_inset
            && let Some(bounds) = self
                .editing_input
                .as_ref()
                .and_then(|input| input.read(cx).text_bounds())
        {
            let keyboard_top = window.viewport_size().height - px(bottom);
            let overflow = bounds.bottom() + px(12.) - keyboard_top;
            if overflow > px(0.) {
                let offset = self.settings_scroll.offset();
                self.settings_scroll
                    .set_offset(point(offset.x, offset.y - overflow));
            }
        }
        self.bottom_inset = bottom;
        let ui = Ui::new(window, cx);
        let dark_icons = std::ptr::eq(ui.colors, &theme::LIGHT);
        if self.dark_system_bar_icons != Some(dark_icons) {
            match activity_action(
                &self.android,
                ActivityAction::Appearance(dark_icons, ui.colors),
            ) {
                Ok(()) => self.dark_system_bar_icons = Some(dark_icons),
                Err(error) => log::warn!("Setting system bar icon appearance: {error}"),
            }
        }
        let devices = matches!(self.page, Page::Devices | Page::PairingMenu);
        let expanded = self.page == Page::PairingMenu;
        let show_add_rule =
            matches!(&self.page, Page::Forwards(_)) && self.can_add_rule(self.forward_filters.device.as_deref());
        let drawer = self.drawer.position(window, cx);
        let content_width = window.viewport_size().width - px(left + right);
        let drawer_width =
            (f32::from(window.viewport_size().width) - left - right - 56.).clamp(0., 304.);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .pl(px(left))
            .pr(px(right))
            .pt(px(top))
            .pb(px(bottom))
            .bg(rgb(ui.colors.canvas.background))
            .font_family(self.ui_font.clone())
            .text_color(rgb(ui.colors.canvas.foreground))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(top + app_bar_height(window)))
                    .bg(rgb(ui.colors.card)),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(self.app_bar(ui, window, cx))
                    .child(match self.page.clone() {
                        Page::Devices | Page::PairingMenu => {
                            self.devices(ui, cx).into_any_element()
                        }
                        Page::Settings => self
                            .settings(None, ui, content_width, cx)
                            .into_any_element(),
                        Page::SettingsCategory(section) => self
                            .settings(Some(section), ui, content_width, cx)
                            .into_any_element(),
                        Page::ForwardEdit { owner, .. } => {
                            self.rule_editor(owner.as_deref(), ui, cx).into_any_element()
                        }
                        Page::Forwards(_) => self
                            .forward_list(ui, window, cx)
                            .into_any_element(),
                        Page::Sessions(_) => self.sessions_page(ui, cx).into_any_element(),
                        Page::Peer(id) => self.peer(&id, ui, cx).into_any_element(),
                        Page::Terminal(id) => self.terminal(id, window, cx).into_any_element(),
                    })
                    .when(expanded, |page| {
                        page.child(
                            div()
                                .id("dismiss-pairing-menu")
                                .absolute()
                                .inset_0()
                                .occlude()
                                .bg(rgba(ui.colors.modal_overlay))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.back(cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(devices && self.client.is_some() && !self.pairing, |page| {
                        page.child(self.pairing_actions(expanded, ui, cx))
                    })
                    .when(show_add_rule, |page| {
                        page.child(floating_corner().child(
                            ui.floating_action("add-forward", Icon::Plus, "Add forwarding rule")
                                .on_click(cx.listener(|this, _, _, cx| this.open_rule(None, cx))),
                        ))
                    })
                    .when(self.drawer.open || drawer > 0., |page| {
                        page.child(self.navigation_drawer(drawer, drawer_width, ui, cx))
                    })
                    .children(Toast::overlay(window, cx)),
            )
    }
}
