use anyhow::Result;
use std::{io, time::Duration};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use paxri_control::terminal::Size;
use windows_sys::Win32::System::Console::*;

// ReadConsoleW preserves Ctrl+Z as PTY input. Rust's standard console reader treats it as EOF.
fn read_units(buffer: &mut [u16]) -> io::Result<usize> {
    let mut count = 0;
    let ok = unsafe {
        ReadConsoleW(
            GetStdHandle(STD_INPUT_HANDLE),
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut count,
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(count as usize)
}

pub(super) struct Input(mpsc::Receiver<io::Result<Vec<u8>>>);

impl Input {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::channel(1);
        // Keep one console read pending across resize notifications.
        tokio::task::spawn_blocking(move || {
            let mut units = [0u16; 2048];
            let mut carry = 0;
            while !tx.is_closed() {
                let count = match read_units(&mut units[carry..]) {
                    Ok(count) => count,
                    Err(error) => {
                        let _ = tx.blocking_send(Err(error));
                        break;
                    }
                };
                let len = carry + count;
                // A supplementary character may arrive in two separate console reads.
                carry = usize::from(count != 0 && matches!(units[len - 1], 0xd800..=0xdbff));
                let end = len - carry;
                let text = String::from_utf16_lossy(&units[..end]);
                if !text.is_empty() && tx.blocking_send(Ok(text.into_bytes())).is_err() {
                    break;
                }
                if count == 0 {
                    break;
                }
                if carry != 0 {
                    units[0] = units[end];
                }
            }
        });
        Self(rx)
    }

    pub(super) async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.0.recv().await.transpose()
    }
}

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
