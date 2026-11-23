# Android GPUI patches

Each directory is named after the Cargo package it modifies. The Android build applies the patches to a copy of the locked upstream source. Rust and Java both build from the patched copy of `gpui-mobile`. [Build script](../build.py) · [Dependency lockfile](../Cargo.lock)

| Package | Patch | Purpose |
| --- | --- | --- |
| `gpui-mobile` | [hardware-shortcuts.patch](gpui-mobile/hardware-shortcuts.patch) | Give the focused view a chance to handle Ctrl shortcuts before forwarding printable input. |
| `gpui-mobile` | [frame-pacing.patch](gpui-mobile/frame-pacing.patch) | Coordinate frame requests with Android vsync during dragging and scrolling. |
| `gpui-mobile` | [ime-composition.patch](gpui-mobile/ime-composition.patch) | Let the app configure the input field and receive composing text and committed input in order. |
| `gpui-pre` | [vertical-scroll.patch](gpui-pre/vertical-scroll.patch) | Apply vertical finger movement immediately while preserving the tap threshold. |
| `gpui-pre-wgpu` | [font-fallback.patch](gpui-pre-wgpu/font-fallback.patch) | Keep symbol-only fonts available and honor the configured fallback order for each character. |

When adding a patch, list it in `PATCH_SERIES` in application order. When updating a dependency, reapply its series to the new upstream source and run `python3 mobile/android/build.py` from the repository root.
