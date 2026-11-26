use super::*;
use paxri_core::connection::{peer_state, unix_time_ms};
use paxri_ui::components::{
    ButtonVariant::{Destructive, Ghost, Secondary},
    DeviceCounts, DropdownState, FLOATING_ACTION_CLEARANCE, Icon, ListState, ListView,
    ModalDialog, ScrollMetrics, ScrollSource, Scrollbar, floating_corner,
};
use paxri_ui::connections::presence_detail;
use paxri_ui::terminal::display;
use paxri_ui::theme::{TextRole, TypographyExt};

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

#[derive(Clone)]
struct TerminalHistory {
    mobile: Entity<Mobile>,
    id: u64,
}

impl ScrollSource for TerminalHistory {
    fn metrics(&self, cx: &App) -> ScrollMetrics {
        self.mobile
            .read(cx)
            .sessions
            .get(&self.id)
            .map_or_else(
                || ScrollMetrics::new(0., 0., 0.),
                |s| s.screen.scroll_metrics(),
            )
    }

    fn scroll_to(&self, offset: f32, cx: &mut App) {
        self.mobile.update(cx, |this, cx| {
            if let Some(s) = this.sessions.get_mut(&self.id)
                && s.screen.scroll_to_offset(offset)
            {
                cx.notify();
            }
        });
    }
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
                "Sessions".into(),
                self.sessions_subtitle(cx),
                origin.as_ref().map(|_| "Back to device details"),
                None,
            ),
            Page::Peer(id) => (
                SharedString::from("Device"),
                None,
                Some("Back to devices"),
                Some(self.device_actions(id, ui, cx)),
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
                self.forwards_subtitle(cx),
                peer.as_ref().map(|_| "Back to device details"),
                None,
            ),
            Page::Settings => (SharedString::from("Settings"), None, None, None),
            Page::SettingsCategory(section) => (
                SharedString::from(section.title),
                section.table.heading_label().map(|label| label.to_string()),
                Some(
                    if matches!(
                        *section,
                        keybar::SECTION | paxri_ui::settings::catalog::SHORTCUTS
                    ) {
                        "Back to terminal controls"
                    } else {
                        "Back to settings"
                    },
                ),
                None,
            ),
            Page::Terminal(id) => {
                let id = *id;
                let session = &self.sessions[&id];
                let (title, subtitle) = session.labels(self.presence.online(&session.peer), cx);
                let action = match session.state {
                    SessionState::Connecting | SessionState::Connected | SessionState::Reconnecting => {
                        let menu = self.session_menu(&session.peer, session.key, title.clone(), true, cx);
                        Some(ui.actions_menu("terminal-actions", "Terminal actions".into(), menu)
                            .circular_trigger().into_any_element())
                    }
                    SessionState::Stopping => None,
                    SessionState::Ended(_) => Some(
                        ui.button(Ghost, "close-session", Icon::Close.draw(20.))
                            .circular()
                            .tooltip(paxri_ui::components::tooltip("Close session".into()))
                            .role(Role::Button)
                            .aria_label("Close session")
                            .on_click(cx.listener(move |this, _, _, cx| this.end_session(id, cx)))
                            .into_any_element(),
                    ),
                };
                (
                    title,
                    Some(subtitle),
                    Some(if matches!(self.terminal_return, Page::Sessions(_)) { "Back to sessions" } else { "Back to device details" }),
                    action,
                )
            }
        };
        let terminal_bell = matches!(self.page, Page::Terminal(id) if self.sessions.get(&id).is_some_and(|s| s.bell.is_ringing()));
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
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .when(terminal_bell, |row| row.child(Icon::Bell.draw(16.)))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_role(TextRole::Title)
                                    .child(title),
                            ),
                    )
                    .when_some(subtitle, |labels, subtitle| {
                        labels.child(ui.caption(subtitle).truncate())
                    }),
            )
            .children(end)
    }

    fn pairing_actions(&self, expanded: bool, ui: Ui, cx: &Context<Self>) -> Div {
        let action = |id: &'static str, label: &'static str, icon| {
            let text = div().h(px(48.)).flex().items_center().child(label);
            ui.button(Ghost, id, text)
                .row()
                .icon(icon)
                .role(Role::Button)
                .aria_label(label)
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
                        .child(
                            action("enter-code", "Enter pairing code", Icon::Pencil).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.navigate(Page::Devices, cx);
                                    this.open_pair_code_modal(window, cx);
                                }),
                            ),
                        )
                        .child(
                            action("scan-qr", "Scan QR code", Icon::ScanQr).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.navigate(Page::Devices, cx);
                                    if let Err(error) =
                                        activity_action(&this.android, ActivityAction::Scan)
                                    {
                                        Toast::show(error, this.window, cx);
                                    }
                                    cx.notify();
                                }),
                            ),
                        ),
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

    fn device_counts(&self, id: &str) -> DeviceCounts {
        let peer = &self.peers[id];
        let sessions = if !peer.supports_shell() {
            Some(0)
        } else if self.presence.online(id) && !self.session_list_errors.contains(id) {
            self.hosted.get(id).map(Vec::len)
        } else { None };
        let forwards = if !peer.supports_forward_rules() { Some(0) }
            else if self.presence.online(id) { self.forwards.count(id) }
            else { None };
        DeviceCounts { sessions, forwards }
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
        let notice = if !peers.is_empty() || self.pairing.is_some() {
            None
        } else if self.starting_network {
            Some(ListState::loading("Starting network…"))
        } else if self.client.is_none() {
            Some(ListState::empty(Icon::Server, "Network unavailable"))
        } else if self.peers.is_empty() {
            Some(ListState::empty(Icon::Monitor, "No paired devices").detail("Tap + to pair a computer."))
        } else {
            Some(ListState::filtered("No matching devices"))
        };
        let show_list = notice.is_none();
        let matched = peers.len();
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
            .when(!self.peers.is_empty(), |page| {
                page.child(
                    filters
                        .controls(|v| &mut v.device_filters, cx)
                        .results(matched, self.peers.len()),
                )
            })
            .when_some(notice, |page, state| page.child(state.render(ui)))
            .when(show_list, |page| page.child(
                ListView::scrollable("paired-devices")
                    .with_floating_action(self.client.is_some() && self.pairing.is_none())
                    .when_some(self.pairing.as_ref(), |list, pending| {
                        let short_id = paxri_config::peer_id::default_prefix(&pending.peer_id);
                        let waiting = pending.cancel.is_some();
                        let row = ui.device_item(
                            paxri_config::DevicePlatform::Unknown,
                            format!("Device {short_id}").into(),
                            false,
                            if waiting { "Waiting for approval" } else { "Canceling…" }.into(),
                        );
                        let row = if waiting {
                            row.trailing(
                                ui.button(Secondary, "cancel-pairing", "Cancel")
                                    .role(Role::Button)
                                    .aria_label("Cancel pairing request")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(cancel) = this.pairing.as_mut().and_then(|p| p.cancel.take()) {
                                            let _ = cancel.send(());
                                            cx.notify();
                                        }
                                    })),
                            )
                        } else {
                            row
                        };
                        list.child(row.inset_row())
                    })
                    .children(rows),
            ))
    }

    fn device_actions(&self, id: &str, ui: Ui, cx: &Context<Self>) -> AnyElement {
        let name = &self.peers[id].name;
        let menu_id = SharedString::from(format!("device-actions-{id}"));
        let id = id.to_owned();
        let view = cx.weak_entity();
        ui.device_actions(menu_id, name, move |_, cx| {
            let _ = view.update(cx, |this, cx| {
                if let Some(peer) = this.peers.get(&id) {
                    this.confirm_unpair(
                        id.clone(),
                        peer.generation.clone(),
                        peer.name.clone(),
                        cx,
                    );
                }
            });
        })
        .into_any_element()
    }

    fn peer(&self, id: &str, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = &self.peers[id];
        let counts = self.device_counts(id);
        let paths = self.presence.paths_for(id);
        let can_connect = self.presence.online(id) && peer.supports_shell();
        let sessions_peer = id.to_owned();
        let forwards_peer = id.to_owned();
        let body = div()
            .px(px(18.))
            .flex()
            .flex_col()
            .gap_4()
            .pt_4()
            .when(can_connect, |page| page.pb(FLOATING_ACTION_CLEARANCE))
            .child(ui.device_heading(
                None,
                peer.name.clone().into(),
                peer.platform,
                Some((id, self.short_peer_id(id))),
            ))
            .child(ui.device_detail_sections(
                self.connection_history.connection_section(id, paths, true, ui),
                counts,
                peer.supports_forward_rules(),
                cx.listener(move |this, _, _, cx| {
                    this.show_sessions(Some(sessions_peer.clone()), cx)
                }),
                cx.listener(move |this, _, _, cx| {
                    this.navigate(Page::Forwards(Some(forwards_peer.clone())), cx);
                    cx.notify();
                }),
            ));
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(paxri_ui::components::ScrollArea::new("device-page", body))
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
        let background = s.screen.sync_appearance(Ui::new(window, cx).colors, &appearance);
        let cursor_shape = s
            .screen
            .cursor_for_paint(s.connected(), !s.composing.text.is_empty(), window, cx);
        let indicator_color = s.screen.palette.surface.foreground;
        let view = cx.entity();
        let sizing = view.clone();
        let history = TerminalHistory {
            mobile: view.clone(),
            id,
        };
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
                sizing.update(cx, |this, cx| {
                    if this.modal.is_some() {
                        this.set_touch_area(0, [0; 5], this.keybar.clone(), false);
                        return;
                    }
                    let touchable = !DropdownState::has_open(cx);
                    this.set_touch_area(id, area, this.keybar.clone(), touchable);
                    if let Some(s) = this.sessions.get_mut(&id) {
                        let grid = GridSize::from_viewport(bounds.size, s.screen.font.cell);
                        if s.screen.resize(grid) {
                            let _ = s.input.send(Frame::Resize(PtySize {
                                rows: grid.rows,
                                cols: grid.cols,
                            }));
                        }
                        let handles = s.handle_positions(scale);
                        let connected = s.connected();
                        if this.selection_handles != Some((id, handles, connected))
                            && activity_action(&this.android, ActivityAction::SelectionHandles(id, handles, connected)).is_ok()
                        {
                            this.selection_handles = Some((id, handles, connected));
                        }
                        // The IME binds to the session registered by set_touch_area above.
                        if this.keyboard_on_entry == Some(id) && s.connected() && touchable
                            && !this.drawer.open && *ACTIVITY_VISIBLE.borrow()
                        {
                            this.keyboard_on_entry = None;
                            this.show_terminal_keyboard(id);
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
                            cursor_shape,
                            None,
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
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(background))
            .child(screen)
            .child(
                Scrollbar::vertical(("terminal-history", id), history).color(indicator_color),
            );
        // The Android touch surface owns the multi-pointer key row beneath the canvas.
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(grid)
            .child(div().h(px(toolbar)).flex_none())
    }
    fn modal_overlay(&self, ui: Ui, cx: &Context<Self>) -> Option<AnyElement> {
        let modal = self.modal.as_ref()?;
        let cancel = cx.listener(|this, _: &(), window, cx| this.dismiss_modal(Some(window), cx));
        let on_cancel = move |window: &mut Window, cx: &mut App| cancel(&(), window, cx);
        let dialog = match modal {
            Modal::Unpair {
                id,
                generation,
                name,
            } => {
                let id = id.clone();
                let generation = generation.clone();
                let confirm = cx.listener(move |this, _: &(), window, cx| {
                    this.dismiss_modal(Some(window), cx);
                    this.unpair_device(id.clone(), generation.clone(), cx);
                });
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!("Unpair this device and {name}?"))
                    .child(ui.muted(
                        "Active connections will close. If the device is offline, it will be notified when reachable.",
                    ));
                ModalDialog::new(
                    "unpair-modal",
                    "Unpair device?",
                    body,
                    "Unpair device",
                    move |window, cx| confirm(&(), window, cx),
                    on_cancel,
                )
                .confirm_variant(Destructive)
            }
            Modal::RemoveForward { owner, name, route } => {
                let owner = owner.clone();
                let rule_name = name.clone();
                let confirm = cx.listener(move |this, _: &(), window, cx| {
                    this.dismiss_modal(Some(window), cx);
                    this.remove_forward(&owner, rule_name.clone(), cx);
                });
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_role(TextRole::Label).child(name.clone()))
                    .child(ui.muted(route.clone()));
                ModalDialog::new(
                    "remove-forward-modal",
                    "Remove forwarding rule?",
                    body,
                    "Remove rule",
                    move |window, cx| confirm(&(), window, cx),
                    on_cancel,
                )
                .confirm_variant(Destructive)
            }
            Modal::PairCode { input } => {
                let submit_input = input.clone();
                let confirm = cx.listener(move |this, _: &(), window, cx| {
                    let code = submit_input.read(cx).text().trim().to_owned();
                    if this.submit_pair_code(code, cx) {
                        this.dismiss_modal(Some(window), cx);
                    }
                });
                ModalDialog::new(
                    "pair-code-modal",
                    "Pair a computer",
                    div().w_full().child(input.clone()),
                    "Pair",
                    move |window, cx| confirm(&(), window, cx),
                    on_cancel,
                )
            }
            Modal::RenameSession { peer, key, input } => {
                let peer = peer.clone();
                let key = *key;
                let submit_input = input.clone();
                let confirm = cx.listener(move |this, _: &(), window, cx| {
                    let name = submit_input.read(cx).text().to_owned();
                    this.dismiss_modal(Some(window), cx);
                    this.rename_session(peer.clone(), key, name, cx);
                });
                ModalDialog::new(
                    "rename-session-modal",
                    "Rename session",
                    div().w_full().child(input.clone()),
                    "Rename",
                    move |window, cx| confirm(&(), window, cx),
                    on_cancel,
                )
            }
            Modal::OpenLink { uri } => {
                let open_uri = uri.clone();
                let copy_uri = uri.clone();
                let confirm = cx.listener(move |this, _: &(), window, cx| {
                    this.dismiss_modal(Some(window), cx);
                    if let Err(error) =
                        activity_action(&this.android, ActivityAction::OpenUrl(&open_uri))
                    {
                        Toast::show(error, this.window, cx);
                    }
                });
                let copy = cx.listener(move |this, _: &(), window, cx| {
                    this.dismiss_modal(Some(window), cx);
                    match gpui_mobile::packages::clipboard::set_text(&copy_uri) {
                        Ok(()) => Toast::show("Link copied", this.window, cx),
                        Err(error) => Toast::show(error, this.window, cx),
                    }
                });
                let body = div()
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .rounded(theme::CONTROL_RADIUS)
                    .border_1()
                    .border_color(rgb(ui.colors.border))
                    .bg(rgb(ui.colors.card))
                    .text_color(rgb(ui.colors.canvas.foreground))
                    .child(paxri_ui::terminal::links::wrappable_uri(uri));
                ModalDialog::new(
                    "open-link-modal",
                    "Open link",
                    body,
                    "Open",
                    move |window, cx| confirm(&(), window, cx),
                    on_cancel,
                )
                .secondary_action("Copy", move |window, cx| copy(&(), window, cx))
            }
        };
        Some(dialog.into_any_element())
    }
}

impl Render for Mobile {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(*SYSTEM_UI_REM.lock().unwrap()));
        let [left, top, right, bottom] = *EDGES.lock().unwrap();
        theme::set_safe_area(
            Edges {
                top: px(top),
                right: px(right),
                bottom: px(bottom),
                left: px(left),
            },
            cx,
        );
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
        let show_add_rule = matches!(&self.page, Page::Forwards(_))
            && self.can_add_rule(self.forward_filters.scope.device.as_deref());
        let drawer = self.drawer.position(window, cx);
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
            .font(self.ui_font.clone())
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
                        Page::Settings => self.settings(None, ui, cx),
                        Page::SettingsCategory(section) => self.settings(Some(section), ui, cx),
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
                    .when(devices && self.client.is_some() && self.pairing.is_none(), |page| {
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
                    .children(self.modal_overlay(ui, cx))
                    .children(Toast::overlay(window, cx)),
            )
    }
}
