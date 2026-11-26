# Linux GPUI patches

This is the published `gpui-pre-linux` 0.3.6 backend from the same Zed snapshot as [`gpui-pre`](../gpui-pre/LOCAL_CHANGES.md).

- `src/linux/touch.rs` tracks direct touchscreen contacts. The Wayland seat and X11 input handlers offer contacts to the terminal before falling back to taps, scrolling, or pinch.
- Wayland and XInput trackpad pinch updates compute relative step ratios (`scale / previous - 1.0`) rather than raw differences, matching touchscreen pinch, macOS, and Windows.
- The X11 feature also enables `gpui/x11`, so the core can choose the X11 backend when only `DISPLAY` is set.
