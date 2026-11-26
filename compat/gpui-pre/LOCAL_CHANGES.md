# GPUI core patches

This is the published `gpui-pre` 0.3.6 package (Zed `bcf6582ce3500df93a8a39366640173e6786cea6`), with its standalone examples and integration test targets omitted. Desktop and Android select this same package through the workspace patch.

- `src/gestures.rs` preserves Android's immediate vertical scrolling while rejecting small movements as taps.
- `src/text_system.rs` and `src/text_system/line_wrapper.rs` share character widths without retaining closed windows' text systems and fonts.
- `src/elements/text.rs` refits truncated single-line labels against the final layout width so tabs, menus, and lists keep their ellipsis.
- `src/window/tooltip.rs` preserves cursor and element anchors, four preferred sides, automatic flipping, and viewport clamping for desktop tooltips. Builders that do not request placement keep the upstream behavior.
- `src/interactive.rs` and `src/window.rs` let the desktop backends offer raw touchscreen contacts to the terminal before promoting unclaimed contacts to pointer or gesture events. Android continues to use GPUI's separate `TouchEvent` path. The terminal can also ask the native backend to hide the pointer until it moves.
