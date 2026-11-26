# Windows GPUI patches

This is the published `gpui-pre-windows` 0.3.6 backend from the same Zed snapshot as [`gpui-pre`](../gpui-pre/LOCAL_CHANGES.md).

- `src/events.rs` sends direct touchscreen contacts to the terminal before Windows promotes unclaimed contacts to mouse or gesture events. Native trackpad gestures still use Direct Manipulation.
- `src/directx_renderer.rs` embeds the HLSL sources used for debug rendering so a preview executable can start without its build checkout.

The desktop app starts this backend directly to keep using its own Windows manifest.
