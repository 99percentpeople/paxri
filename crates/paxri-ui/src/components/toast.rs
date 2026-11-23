use super::Ui;
use crate::theme::{CONTROL_RADIUS, SPACE_2, SPACE_3, SPACE_4};
use gpui::{prelude::*, *};
use std::{collections::HashMap, time::Duration};

#[derive(Default)]
pub struct Toast {
    visible: HashMap<AnyWindowHandle, (SharedString, Task<()>)>,
    #[cfg(target_os = "android")]
    native: Option<Box<dyn Fn(&str) -> bool>>,
}

impl Global for Toast {}

impl Toast {
    #[cfg(target_os = "android")]
    pub fn set_native(show: impl Fn(&str) -> bool + 'static, cx: &mut App) {
        cx.default_global::<Self>().native = Some(Box::new(show));
    }

    pub fn show(message: impl Into<SharedString>, target: AnyWindowHandle, cx: &mut App) {
        let message = message.into();
        #[cfg(target_os = "android")]
        if cx
            .try_global::<Self>()
            .and_then(|s| s.native.as_ref())
            .is_some_and(|show| show(&message))
        {
            return;
        }

        let timer = cx.background_executor().timer(Duration::from_secs(3));
        let dismiss = cx.spawn(async move |cx| {
            timer.await;
            let _ = cx.update(|cx| {
                cx.default_global::<Self>().visible.remove(&target);
                let _ = target.update(cx, |_, window, _| window.refresh());
            });
        });
        // Replacing the task also cancels the previous message's timeout.
        cx.default_global::<Self>()
            .visible
            .insert(target, (message, dismiss));
        cx.defer(move |cx| {
            let _ = target.update(cx, |_, window, _| window.refresh());
        });
    }

    pub fn overlay(window: &Window, cx: &App) -> Option<Div> {
        let (message, _) = cx
            .try_global::<Self>()?
            .visible
            .get(&window.window_handle())?;
        let colors = &Ui::new(window, cx).colors.primary;
        let layer = div().absolute();
        let layer = if cfg!(target_os = "android") {
            layer.bottom(SPACE_4)
        } else {
            layer.top(SPACE_3)
        };
        Some(
            layer
                .left_0()
                .right_0()
                .px(SPACE_4)
                .flex()
                .justify_center()
                .child(
                    div()
                        .min_w_0()
                        .max_w(px(400.))
                        .px(SPACE_3)
                        .py(SPACE_2)
                        .rounded(CONTROL_RADIUS)
                        .shadow_md()
                        .bg(rgb(colors.background))
                        .text_color(rgb(colors.foreground))
                        .text_sm()
                        .child(message.clone()),
                ),
        )
    }
}
