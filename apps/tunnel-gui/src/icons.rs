use gpui::{prelude::*, *};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Home,
    Settings,
    ChevronDown,
    ChevronUp,
    TabsOnLeft,
    TabsOnTop,
    Terminal,
    Plus,
}

impl Icon {
    /// Draw a monochrome icon on a 16-point grid using the element's text color.
    pub fn draw(self, size: f32) -> impl IntoElement {
        const FRAME: &[(f32, f32)] = &[
            (1.5, 2.5),
            (14.5, 2.5),
            (14.5, 13.5),
            (1.5, 13.5),
            (1.5, 2.5),
        ];
        let strokes: &[&[(f32, f32)]] = match self {
            Self::Home => &[
                &[(1., 7.4), (8., 1.8), (15., 7.4)],
                &[
                    (3., 6.),
                    (3., 14.),
                    (6.5, 14.),
                    (6.5, 9.),
                    (9.5, 9.),
                    (9.5, 14.),
                    (13., 14.),
                    (13., 6.),
                ],
            ],
            Self::Settings => &[
                &[(1., 5.), (4., 5.)],
                &[(7.5, 5.), (15., 5.)],
                &[(4., 3.), (7.5, 3.), (7.5, 7.), (4., 7.), (4., 3.)],
                &[(1., 11.), (9., 11.)],
                &[(12.5, 11.), (15., 11.)],
                &[(9., 9.), (12.5, 9.), (12.5, 13.), (9., 13.), (9., 9.)],
            ],
            Self::ChevronDown => &[&[(3.5, 6.), (8., 10.5), (12.5, 6.)]],
            Self::ChevronUp => &[&[(3.5, 10.), (8., 5.5), (12.5, 10.)]],
            Self::TabsOnLeft => &[FRAME, &[(5.5, 2.5), (5.5, 13.5)]],
            Self::TabsOnTop => &[FRAME, &[(1.5, 6.), (14.5, 6.)]],
            Self::Terminal => &[
                &[(2., 3.5), (6.5, 8.), (2., 12.5)],
                &[(8., 12.5), (14., 12.5)],
            ],
            Self::Plus => &[&[(8., 2.5), (8., 13.5)], &[(2.5, 8.), (13.5, 8.)]],
        };
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let scale = size / 16.;
                let mut path = PathBuilder::stroke(px(1.4 * scale));
                for stroke in strokes {
                    if let Some((&(x, y), rest)) = stroke.split_first() {
                        path.move_to(point(px(x), px(y)));
                        for &(x, y) in rest {
                            path.line_to(point(px(x), px(y)));
                        }
                    }
                }
                path.scale(scale);
                path.translate(bounds.origin);
                if let Ok(path) = path.build() {
                    window.paint_path(path, window.text_style().color);
                }
            },
        )
        .size(px(size))
        .flex_none()
    }

    pub fn label(self, text: impl Into<SharedString>) -> Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.draw(16.))
            .child(text.into())
    }
}
