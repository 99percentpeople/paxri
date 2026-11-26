use crate::{Bounds, Pixels, Point, Size, point, px, size};

/// What a tooltip is anchored to when positioned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipAnchor {
    /// Positioned relative to the pointer location (default).
    #[default]
    Cursor,
    /// Positioned relative to the hovered element's bounding box.
    Element,
}

/// Preferred placement of a tooltip relative to its anchor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipPlacement {
    /// Centered below the anchor, flipping above if clipped.
    #[default]
    Bottom,
    /// Centered above the anchor, flipping below if clipped.
    Top,
    /// Centered to the left of the anchor, flipping right if clipped.
    Left,
    /// Centered to the right of the anchor, flipping left if clipped.
    Right,
}

const TOOLTIP_VIEWPORT_MARGIN: Pixels = px(8.);

fn clamp_tooltip_axis(value: Pixels, size: Pixels, limit: Pixels, margin: Pixels) -> Pixels {
    let max = (limit - size - margin).max(Pixels::ZERO);
    let min = margin.min(max);
    value.clamp(min, max)
}

/// Compute the top-left origin for a tooltip given its anchor mode, element bounds, pointer
/// position, preferred placement, and gap.
pub fn place_tooltip(
    tooltip_size: Size<Pixels>,
    window_size: Size<Pixels>,
    anchor: TooltipAnchor,
    anchor_bounds: Option<Bounds<Pixels>>,
    mouse_position: Point<Pixels>,
    placement: TooltipPlacement,
    offset: Pixels,
) -> Point<Pixels> {
    let margin = TOOLTIP_VIEWPORT_MARGIN;
    let (target, leading_offset) = match (anchor, anchor_bounds) {
        (TooltipAnchor::Element, Some(bounds)) => (bounds, offset),
        _ => {
            // Standard pointers extend downward/rightward from the hotspot, so leading edges
            // above/left of the hotspot need less gap than trailing edges below/right.
            let leading = (offset / 2.).max(px(8.)).min(offset);
            (Bounds::new(mouse_position, size(Pixels::ZERO, Pixels::ZERO)), leading)
        }
    };

    let centered_x = target.center().x - tooltip_size.width / 2.;
    let centered_y = target.center().y - tooltip_size.height / 2.;
    let below_y = target.bottom() + offset;
    let above_y = target.top() - tooltip_size.height - leading_offset;
    let right_x = target.right() + offset;
    let left_x = target.left() - tooltip_size.width - leading_offset;

    let fits_below = below_y + tooltip_size.height <= window_size.height - margin;
    let fits_above = above_y >= margin;
    let fits_right = right_x + tooltip_size.width <= window_size.width - margin;
    let fits_left = left_x >= margin;

    let (x, y) = match placement {
        TooltipPlacement::Bottom => {
            let y = if fits_below {
                below_y
            } else if fits_above || target.top() > window_size.height - target.bottom() {
                above_y
            } else {
                below_y
            };
            (centered_x, y)
        }
        TooltipPlacement::Top => {
            let y = if fits_above {
                above_y
            } else if fits_below || window_size.height - target.bottom() > target.top() {
                below_y
            } else {
                above_y
            };
            (centered_x, y)
        }
        TooltipPlacement::Left => {
            let x = if fits_left {
                left_x
            } else if fits_right || window_size.width - target.right() > target.left() {
                right_x
            } else {
                left_x
            };
            (x, centered_y)
        }
        TooltipPlacement::Right => {
            let x = if fits_right {
                right_x
            } else if fits_left || target.left() > window_size.width - target.right() {
                left_x
            } else {
                right_x
            };
            (x, centered_y)
        }
    };

    point(
        clamp_tooltip_axis(x, tooltip_size.width, window_size.width, margin),
        clamp_tooltip_axis(y, tooltip_size.height, window_size.height, margin),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_anchor_flips_up_and_clamps_to_the_left_edge() {
        let anchor = Bounds::new(point(px(8.), px(150.)), size(px(32.), px(24.)));
        assert_eq!(
            place_tooltip(
                size(px(100.), px(30.)),
                size(px(300.), px(200.)),
                TooltipAnchor::Element,
                Some(anchor),
                point(px(22.), px(169.)),
                TooltipPlacement::Bottom,
                px(8.),
            ),
            point(px(8.), px(112.)),
        );
    }

    #[test]
    fn cursor_anchor_flips_left_with_a_smaller_hotspot_gap() {
        assert_eq!(
            place_tooltip(
                size(px(72.), px(28.)),
                size(px(300.), px(200.)),
                TooltipAnchor::Cursor,
                None,
                point(px(280.), px(100.)),
                TooltipPlacement::Right,
                px(18.),
            ),
            point(px(199.), px(86.)),
        );
    }
}
