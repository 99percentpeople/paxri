# Android GPUI patches

Each directory is named after the Cargo package it modifies. The Android build applies the patches to a copy of the locked upstream source. Rust and Java both build from the patched copy of `gpui-mobile`. [Build script](../build.py) · [Dependency lockfile](../../../Cargo.lock)

| Package | Patch | Purpose |
| --- | --- | --- |
| `gpui-mobile` | [hardware-shortcuts.patch](gpui-mobile/hardware-shortcuts.patch) | Give the focused view a chance to handle Ctrl shortcuts before forwarding printable input. |
| `gpui-mobile` | [frame-pacing.patch](gpui-mobile/frame-pacing.patch) | Coordinate frame requests with Android vsync during dragging and scrolling. |
| `gpui-mobile` | [ime-composition.patch](gpui-mobile/ime-composition.patch) | Let the app configure the input field and receive composing text and committed input in order. |
| `gpui-mobile` | [activity-recreation.patch](gpui-mobile/activity-recreation.patch) | Bind the dispatcher and frame clock to the current NativeActivity so its window can reopen in the same process. |
| `gpui-mobile` | [system-appearance.patch](gpui-mobile/system-appearance.patch) | Read Android night mode when opening a window so its first frame uses the current system theme. |
| `gpui-pre` | [vertical-scroll.patch](gpui-pre/vertical-scroll.patch) | Apply vertical finger movement immediately while preserving the tap threshold. |
| `gpui-pre` | [text-system-lifetime.patch](gpui-pre/text-system-lifetime.patch) | Reuse character widths while allowing a closed window's text system and fonts to be freed. |
| `gpui-pre-wgpu` | [font-fallback.patch](gpui-pre-wgpu/font-fallback.patch) | Keep symbol-only fonts available, detect color emoji fonts by font tables, match closest available fallback weight, and honor the configured fallback order for each character. |

When adding a patch, list it in `PATCH_SERIES` in application order. When updating a dependency, reapply its series to the new upstream source and run `python3 mobile/android/build.py` from the repository root.
