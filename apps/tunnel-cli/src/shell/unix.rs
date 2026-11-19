use anyhow::Result;
use rustix::termios::{self, OptionalActions, Termios};
use std::io;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tunnel_control::terminal::Size;

pub(super) fn size() -> Result<Size> {
    let size = termios::tcgetwinsize(io::stdout())?;
    Size {
        rows: size.ws_row.clamp(1, 1024),
        cols: size.ws_col.clamp(1, 1024),
    }
    .validate()
}

pub(super) struct RawMode(Termios);

impl RawMode {
    pub(super) fn enter() -> Result<Self> {
        let original = termios::tcgetattr(io::stdin())?;
        let mut raw = original.clone();
        raw.make_raw();
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &raw)?;
        Ok(Self(original))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(io::stdin(), OptionalActions::Now, &self.0);
    }
}

pub(super) struct Resize(Signal);
impl Resize {
    pub(super) fn new() -> Result<Self> {
        Ok(Self(signal(SignalKind::window_change())?))
    }
    pub(super) async fn next(&mut self) -> Result<()> {
        self.0
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("window notifications ended"))
    }
}
