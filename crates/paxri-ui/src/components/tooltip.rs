use super::*;

const MAX_TOOLTIP_WIDTH: Pixels = px(320.);
/// Default gap below the pointer hotspot so the tooltip clears the cursor cleanly.
pub const TOOLTIP_CURSOR_OFFSET: Pixels = px(18.);
/// Default gap between an element-anchored trigger edge and its tooltip.
pub const TOOLTIP_ELEMENT_OFFSET: Pixels = px(6.);

/// What a tooltip is positioned relative to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipAnchor {
    /// Positioned relative to the pointer location (default).
    #[default]
    Cursor,
    /// Positioned relative to the hovered element's bounding box.
    Element,
}

/// Preferred tooltip position relative to its anchor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipPlacement {
    /// Centered below the anchor (default), flipping above near the bottom edge.
    #[default]
    Bottom,
    /// Centered above the anchor, flipping below near the top edge.
    Top,
    /// Centered to the left of the anchor, flipping right near the left edge.
    Left,
    /// Centered to the right of the anchor, flipping left near the right edge.
    Right,
}

/// Configuration for tooltip anchor mode, side, and distance.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TooltipOptions {
    pub anchor: TooltipAnchor,
    pub placement: TooltipPlacement,
    pub offset: Option<Pixels>,
}

impl TooltipOptions {
    /// Follow the pointer location (centered below by default).
    pub const fn cursor() -> Self {
        Self {
            anchor: TooltipAnchor::Cursor,
            placement: TooltipPlacement::Bottom,
            offset: None,
        }
    }

    /// Anchor to the hovered element's bounds (centered below by default).
    pub const fn element() -> Self {
        Self {
            anchor: TooltipAnchor::Element,
            placement: TooltipPlacement::Bottom,
            offset: None,
        }
    }

    pub const fn placement(mut self, placement: TooltipPlacement) -> Self {
        self.placement = placement;
        self
    }

    pub const fn offset(mut self, offset: Pixels) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn resolved_offset(self) -> Pixels {
        self.offset.unwrap_or(match self.anchor {
            TooltipAnchor::Cursor => TOOLTIP_CURSOR_OFFSET,
            TooltipAnchor::Element => TOOLTIP_ELEMENT_OFFSET,
        })
    }
}

#[cfg(not(target_os = "android"))]
impl From<TooltipAnchor> for gpui::TooltipAnchor {
    fn from(value: TooltipAnchor) -> Self {
        match value {
            TooltipAnchor::Cursor => Self::Cursor,
            TooltipAnchor::Element => Self::Element,
        }
    }
}

#[cfg(not(target_os = "android"))]
impl From<TooltipPlacement> for gpui::TooltipPlacement {
    fn from(value: TooltipPlacement) -> Self {
        match value {
            TooltipPlacement::Bottom => Self::Bottom,
            TooltipPlacement::Top => Self::Top,
            TooltipPlacement::Left => Self::Left,
            TooltipPlacement::Right => Self::Right,
        }
    }
}

/// Build a tooltip centered directly below the pointer with the default offset.
pub fn tooltip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView {
    tooltip_with(text, TooltipOptions::default())
}

/// Build a tooltip at an explicit placement relative to the pointer.
pub fn tooltip_at(
    text: SharedString,
    placement: TooltipPlacement,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    tooltip_with(text, TooltipOptions::default().placement(placement))
}

/// Build a tooltip with explicit anchor (`Cursor` or `Element`), placement, and optional offset.
pub fn tooltip_with(
    text: SharedString,
    options: TooltipOptions,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let offset = options.resolved_offset();
    move |window, cx| {
        #[cfg(not(target_os = "android"))]
        window.set_tooltip_placement(options.anchor.into(), options.placement.into(), offset);
        #[cfg(target_os = "android")]
        let _ = (window, options, offset);
        cx.new(|_| Tooltip(text.clone())).into()
    }
}

struct Tooltip(SharedString);

impl Tooltip {
    fn measured_content_width(&self, max_content_width: Pixels, window: &Window) -> Pixels {
        let font_size = TextRole::Caption.size().to_pixels(window.rem_size());
        let mut style = window.text_style();
        style.font_weight = TextRole::Caption.weight();
        let widest = self
            .0
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    px(0.)
                } else {
                    let run = style.to_run(line.len());
                    window
                        .text_system()
                        .shape_line(line.to_owned().into(), font_size, &[run], None)
                        .width
                }
            })
            .fold(px(0.), Pixels::max);
        // Reserve 1px for subpixel rounding so single-line tips stay unwrapped while
        // wrapped lines receive a definite width for accurate multi-line height layout.
        (widest.ceil() + px(1.)).min(max_content_width)
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Ui::new(window, cx).colors;
        let max_width = MAX_TOOLTIP_WIDTH.min((window.viewport_size().width - SPACE_4).max(px(1.)));
        let padding_x = SPACE_2 * 2.;
        let max_content_width = (max_width - padding_x).max(px(1.));
        let content_width = self.measured_content_width(max_content_width, window);
        let box_width = (content_width + padding_x).min(max_width);
        let container = div()
            .w(box_width)
            .px(SPACE_2)
            .py(SPACE_1)
            .rounded(CONTROL_RADIUS)
            .bg(rgb(colors.primary.background))
            .text_color(rgb(colors.primary.foreground))
            .text_role(TextRole::Caption);
        if self.0.contains('\n') {
            container
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .children(
                    self.0
                        .split('\n')
                        .map(|line| div().w_full().child(line.to_owned())),
                )
        } else {
            container.child(self.0.clone())
        }
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::{
        TOOLTIP_CURSOR_OFFSET, TOOLTIP_ELEMENT_OFFSET, TooltipAnchor, TooltipOptions,
        TooltipPlacement,
    };
    use gpui::{Bounds, point, px, size};

    #[test]
    fn tooltip_supports_cursor_and_element_anchors_with_placements_and_edge_clamping() {
        let win = size(px(520.), px(376.));
        let tip = size(px(160.), px(40.));
        let element = Bounds::new(point(px(100.), px(80.)), size(px(200.), px(24.)));
        let cursor = point(px(260.), px(90.));

        let default = TooltipOptions::default();
        assert_eq!(default.anchor, TooltipAnchor::Cursor);
        assert_eq!(default.placement, TooltipPlacement::Bottom);
        assert_eq!(default.resolved_offset(), TOOLTIP_CURSOR_OFFSET);

        // Default: centered directly below cursor (260 - 80 = 180, 90 + 18 = 108)
        assert_eq!(
            gpui::place_tooltip(
                tip,
                win,
                default.anchor.into(),
                Some(element),
                cursor,
                default.placement.into(),
                default.resolved_offset(),
            ),
            point(px(180.), px(108.))
        );

        // Explicit element anchor: centered below element (200 - 80 = 120, 104 + 6 = 110)
        let by_element = TooltipOptions::element();
        assert_eq!(by_element.resolved_offset(), TOOLTIP_ELEMENT_OFFSET);
        assert_eq!(
            gpui::place_tooltip(
                tip,
                win,
                by_element.anchor.into(),
                Some(element),
                cursor,
                by_element.placement.into(),
                by_element.resolved_offset(),
            ),
            point(px(120.), px(110.))
        );

        // Explicit cursor Top / Left / Right
        assert_eq!(
            gpui::place_tooltip(
                tip,
                win,
                TooltipAnchor::Cursor.into(),
                Some(element),
                cursor,
                TooltipPlacement::Top.into(),
                TOOLTIP_CURSOR_OFFSET,
            ),
            point(px(180.), px(41.))
        );
        assert_eq!(
            gpui::place_tooltip(
                tip,
                win,
                TooltipAnchor::Cursor.into(),
                Some(element),
                cursor,
                TooltipPlacement::Left.into(),
                TOOLTIP_CURSOR_OFFSET,
            ),
            point(px(91.), px(70.))
        );
        assert_eq!(
            gpui::place_tooltip(
                tip,
                win,
                TooltipAnchor::Cursor.into(),
                Some(element),
                cursor,
                TooltipPlacement::Right.into(),
                TOOLTIP_CURSOR_OFFSET,
            ),
            point(px(278.), px(70.))
        );

        // Near bottom-right corner flips above and clamps inside 8px viewport margin.
        assert_eq!(
            gpui::place_tooltip(
                tip,
                win,
                TooltipAnchor::Cursor.into(),
                Some(element),
                point(px(470.), px(350.)),
                TooltipPlacement::Bottom.into(),
                TOOLTIP_CURSOR_OFFSET,
            ),
            point(px(352.), px(301.))
        );
    }
}
