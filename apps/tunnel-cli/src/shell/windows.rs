use anyhow::Result;
use std::{io, time::Duration};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tunnel_control::terminal::Size;
use windows_sys::Win32::System::Console::*;

fn mode(id: STD_HANDLE) -> io::Result<u32> {
    let mut value = 0;
    if unsafe { GetConsoleMode(GetStdHandle(id), &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}
fn set_mode(id: STD_HANDLE, value: u32) -> io::Result<()> {
    if unsafe { SetConsoleMode(GetStdHandle(id), value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) struct RawMode {
    input: u32,
    output: u32,
}
impl RawMode {
    pub(super) fn enter() -> Result<Self> {
        let old = Self {
            input: mode(STD_INPUT_HANDLE)?,
            output: mode(STD_OUTPUT_HANDLE)?,
        };
        set_mode(
            STD_OUTPUT_HANDLE,
            old.output
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                | ENABLE_PROCESSED_OUTPUT
                | DISABLE_NEWLINE_AUTO_RETURN,
        )?;
        set_mode(
            STD_INPUT_HANDLE,
            (old.input
                & !(ENABLE_LINE_INPUT
                    | ENABLE_ECHO_INPUT
                    | ENABLE_PROCESSED_INPUT
                    | ENABLE_QUICK_EDIT_MODE))
                | ENABLE_EXTENDED_FLAGS
                | ENABLE_VIRTUAL_TERMINAL_INPUT,
        )?;
        Ok(old)
    }
}
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = set_mode(STD_INPUT_HANDLE, self.input);
        let _ = set_mode(STD_OUTPUT_HANDLE, self.output);
    }
}

pub(super) fn size() -> Result<Size> {
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let rect = info.srWindow;
    Ok(Size {
        rows: (i32::from(rect.Bottom) - i32::from(rect.Top) + 1).clamp(1, 1024) as u16,
        cols: (i32::from(rect.Right) - i32::from(rect.Left) + 1).clamp(1, 1024) as u16,
    })
}

// ReadConsole's VT input mode consumes resize records. Sample the viewport
// independently so idle full-screen applications also receive size changes.
pub(super) struct Resize(tokio::time::Interval);
impl Resize {
    pub(super) fn new() -> Result<Self> {
        Ok(Self(tokio::time::interval(Duration::from_millis(200))))
    }
    pub(super) async fn next(&mut self) -> Result<()> {
        self.0.tick().await;
        Ok(())
    }
}

// Windows console stdout accepts UTF-8; a PTY packet can split a code point.
pub(super) async fn write_console(
    out: &mut (impl AsyncWrite + Unpin),
    pending: &mut Vec<u8>,
    bytes: &[u8],
) -> Result<()> {
    pending.extend_from_slice(bytes);
    let mut rest = pending.as_slice();
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(_) => {
                out.write_all(rest).await?;
                rest = &[];
            }
            Err(e) => {
                let (valid, tail) = rest.split_at(e.valid_up_to());
                out.write_all(valid).await?;
                let Some(invalid) = e.error_len() else {
                    rest = tail;
                    break;
                };
                out.write_all("�".as_bytes()).await?;
                rest = &tail[invalid..];
            }
        }
    }
    let written = pending.len() - rest.len();
    pending.drain(..written);
    Ok(())
}
