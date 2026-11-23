use super::*;

pub fn tooltip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView {
    move |_, cx| cx.new(|_| Tooltip(text.clone())).into()
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Ui::new(window, cx).colors;
        div()
            .max_w(px(360.).min((window.viewport_size().width - SPACE_4).max(px(1.))))
            .px(SPACE_2)
            .py(SPACE_1)
            .rounded(CONTROL_RADIUS)
            .bg(rgb(colors.primary.background))
            .text_color(rgb(colors.primary.foreground))
            .text_sm()
            .child(self.0.clone())
    }
}
