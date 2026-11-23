This is the published gpui 0.2.2 crate from Zed, with its example and integration test targets omitted. The desktop app uses a local Cargo patch to receive native pinch events. Windows and macOS translate platform magnification events. X11 and Wayland track touchscreen contacts per window. Android uses its own GPUI dependency and Android gesture detector.

Windows debug builds embed the HLSL sources so a standalone executable can initialize DirectWrite after the build checkout is removed. Launch errors include the underlying cause.
