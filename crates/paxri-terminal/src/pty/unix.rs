//! Wake the PTY reader when the shell exits, including when a background job inherited the slave.
use anyhow::{Context, Result};
use portable_pty::MasterPty;
use rustix::event::{PollFd, PollFlags, poll};
use std::{
    fs::File,
    io::{self, Read},
    os::{fd::BorrowedFd, unix::net::UnixStream},
};

pub(super) fn clone_master(master: &dyn MasterPty) -> Result<File> {
    let fd = master
        .as_raw_fd()
        .context("missing native PTY descriptor")?;
    // SAFETY: master owns fd for the duration of the borrow. The caller owns its duplicate.
    let file = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
    Ok(file.into())
}

pub(super) struct Reader {
    master: File,
    exited: UnixStream,
    remaining_tail: usize,
}

impl Reader {
    pub(super) fn new(master: &dyn MasterPty) -> Result<(Self, UnixStream)> {
        let master = clone_master(master)?;
        let (exited, notify) = UnixStream::pair()?;
        Ok((
            Self {
                master,
                exited,
                remaining_tail: 1024 * 1024,
            },
            notify,
        ))
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut fds = [
            PollFd::new(&self.master, PollFlags::IN),
            PollFd::new(&self.exited, PollFlags::IN),
        ];
        poll(&mut fds, None)?;
        if fds[1].revents().is_empty() {
            return self.master.read(buf);
        }
        // Drain already queued output. An inherited writer may keep producing indefinitely.
        if fds[0].revents().is_empty() || self.remaining_tail == 0 {
            return Ok(0);
        }
        let len = buf.len().min(self.remaining_tail);
        let read = self.master.read(&mut buf[..len])?;
        self.remaining_tail -= read;
        Ok(read)
    }
}
