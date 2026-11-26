use anyhow::{Context, Result, bail};
use paxri_config::Config;
use paxri_terminal::{self as wire, pipe, pty::CommandBuilder};
use std::{io, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::watch,
};

struct Process {
    child: Child,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

impl Process {
    fn spawn(builder: CommandBuilder) -> Result<Self> {
        let (program, args) = builder
            .get_argv()
            .split_first()
            .context("missing pipe program")?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .envs(builder.iter_full_env_as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Keep native (possibly non-UTF-8) session paths intact.
        for key in ["PATH", "PAXRI_IPC"] {
            if let Some(value) = builder.get_env(key) {
                cmd.env(key, value);
            }
        }
        if let Some(home) = builder
            .get_cwd()
            .map(std::path::PathBuf::from)
            .or_else(std::env::home_dir)
        {
            cmd.current_dir(home);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let child = cmd.spawn()?;
        #[cfg(windows)]
        let job = super::windows::pipe_job(&child)?;
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // The leader is reaped only after both output pipes drain. Its group can
        // still contain a foreground program launched through a shell.
        if let Some(_id) = self.child.id() {
            #[cfg(unix)]
            if let Some(pid) = rustix::process::Pid::from_raw(_id as i32) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            #[cfg(windows)]
            unsafe {
                use std::os::windows::io::AsRawHandle;
                windows_sys::Win32::System::JobObjects::TerminateJobObject(
                    self.job.as_raw_handle(),
                    1,
                );
            }
        }
    }
}

pub(super) async fn serve(
    builder: Result<CommandBuilder>,
    mut recv: impl AsyncRead + Unpin,
    mut send: impl AsyncWrite + Unpin,
    mut config: watch::Receiver<Arc<Config>>,
    peer: &str,
) -> Result<()> {
    let started = {
        let cfg = config.borrow_and_update();
        if cfg.is_paired(peer) {
            builder.and_then(Process::spawn).map_err(|e| {
                eprintln!("start piped command: {e:#}");
                wire::UNAVAILABLE
            })
        } else {
            Err(wire::DENIED)
        }
    };
    let mut process = match started {
        Ok(process) => process,
        Err(code) => {
            send.write_u8(code).await?;
            send.shutdown().await?;
            return Ok(());
        }
    };
    send.write_u8(wire::READY).await?;
    send.flush().await?;
    let stdin = process
        .child
        .stdin
        .take()
        .context("missing command stdin")?;
    tokio::select! {
        biased;
        _ = config.wait_for(|cfg| !cfg.is_paired(peer)) => Ok(()),
        result = drain_output(&mut process.child, &mut send) => result,
        result = forward_input(&mut recv, stdin) => result,
    }
}

async fn forward_input(input: &mut (impl AsyncRead + Unpin), stdin: ChildStdin) -> Result<()> {
    let mut stdin = Some(stdin);
    let mut buf = vec![0; wire::MAX_DATA];
    loop {
        let n = pipe::read_chunk(input, &mut buf).await?;
        if n == 0 {
            drop(stdin);
            // EOF on the transport itself cancels a command even after stdin EOF.
            input.read_u8().await?;
            bail!("unexpected data after stdin EOF");
        }
        if let Some(dest) = &mut stdin
            && let Err(e) = dest.write_all(&buf[..n]).await
        {
            if e.kind() != io::ErrorKind::BrokenPipe {
                return Err(e.into());
            }
            stdin = None;
        }
    }
}

async fn drain_output(child: &mut Child, send: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
    let mut stdout = child.stdout.take().context("missing command stdout")?;
    let mut stderr = child.stderr.take().context("missing command stderr")?;
    let (mut out_open, mut err_open) = (true, true);
    let mut out = vec![0; wire::MAX_DATA];
    let mut err = vec![0; wire::MAX_DATA];
    while out_open || err_open {
        let (kind, bytes) = tokio::select! {
            n = stdout.read(&mut out), if out_open => (pipe::STDOUT, &out[..n?]),
            n = stderr.read(&mut err), if err_open => (pipe::STDERR, &err[..n?]),
        };
        if bytes.is_empty() {
            if kind == pipe::STDOUT {
                out_open = false
            } else {
                err_open = false
            }
        } else {
            send.write_u8(kind).await?;
            pipe::write_chunk(send, bytes).await?;
        }
    }
    let status = child.wait().await?;
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    };
    #[cfg(windows)]
    let code = status.code().unwrap_or(1);
    send.write_u8(pipe::EXIT).await?;
    send.write_u32(code as u32).await?;
    send.shutdown().await?;
    Ok(())
}
