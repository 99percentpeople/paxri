use super::*;
use peerhatch_ui::components::{ButtonVariant::Ghost, Icon, heading};
use std::time::Instant;

#[derive(Default)]
pub(super) struct Drawer {
    pub(super) open: bool,
    transition: Option<(Instant, f32)>,
}

impl Drawer {
    fn fraction(&self) -> f32 {
        let target = if self.open { 1. } else { 0. };
        let Some((start, from)) = self.transition else {
            return target;
        };
        let remaining = (1. - start.elapsed().as_secs_f32() / 0.20).clamp(0., 1.);
        target + (from - target) * remaining.powi(3)
    }

    pub(super) fn set_open(&mut self, open: bool) {
        if self.open != open {
            self.transition = Some((Instant::now(), self.fraction()));
            self.open = open;
        }
    }

    pub(super) fn position(&self, window: &Window, cx: &App) -> f32 {
        let target = if self.open { 1. } else { 0. };
        if cx.reduce_motion() {
            return target;
        }
        let position = self.fraction();
        if position != target {
            window.request_animation_frame();
        }
        position
    }
}

impl Mobile {
    pub(super) fn navigation_drawer(
        &self,
        position: f32,
        width: f32,
        ui: Ui,
        cx: &Context<Self>,
    ) -> Div {
        let close = || {
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.drawer.set_open(false);
                this.sync_back();
                cx.notify();
            })
        };
        let destination = |id, label: &'static str, icon, page: Page| {
            let selected = self.page == page;
            ui.button(
                Ghost,
                id,
                div().h(px(48.)).flex().items_center().child(label),
            )
            .row()
            .icon(icon)
            .pressed(selected)
            .role(Role::Button)
            .aria_label(label)
            .aria_selected(selected)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.navigate(page.clone(), cx);
                cx.notify();
            }))
        };
        let scrim = div()
            .id("navigation-scrim")
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(ui.colors.modal_overlay).opacity(position))
            .on_click(close());
        let panel = div()
            .id("navigation-drawer")
            .role(Role::Navigation)
            .aria_label("Navigation")
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(width * (position - 1.)))
            .w(px(width))
            .occlude()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .bg(rgb(ui.colors.card))
            .border_r_1()
            .border_color(rgb(ui.colors.border))
            .flex()
            .flex_col()
            .p_3()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl_3()
                    .child(heading("PeerHatch"))
                    .child(
                        ui.icon_button(
                            Ghost,
                            "close-navigation",
                            Icon::Close,
                            "Close navigation menu",
                        )
                        .role(Role::Button)
                        .aria_label("Close navigation menu")
                        .on_click(close()),
                    ),
            )
            .child(ui.separator())
            .child(destination(
                "nav-devices",
                "Devices",
                Icon::Monitor,
                Page::Devices,
            ))
            .child(destination(
                "nav-settings",
                "Settings",
                Icon::Settings,
                Page::Settings,
            ));
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .child(scrim)
            .child(panel)
    }
}
