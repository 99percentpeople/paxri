use super::{
    Duplex, MAX_DEPTH, Options, Progress, Report, Stage,
    paths::{self, Destination},
    wire::{self, Attributes, Message},
};
use anyhow::{Context, Result, bail, ensure};
use std::{io::ErrorKind, path::Path};
use tokio::{fs, io::AsyncWriteExt};

fn entry_name(msg: &Message) -> Result<&str> {
    let name = match msg {
        Message::File { name, .. } | Message::Directory { name, .. } => name,
        _ => bail!("Expected a file or directory"),
    };
    paths::validate_name(name)?;
    Ok(name)
}

pub async fn tree(
    stream: &mut impl Duplex,
    dest: Destination,
    expected: Option<&str>,
    opts: Options,
    report: &mut Report<'_>,
) -> Result<()> {
    let msg = wire::read(stream).await?;
    let name = entry_name(&msg)?;
    ensure!(
        expected.is_none_or(|n| n == name),
        "Peer returned an unexpected file name: {name:?}"
    );
    entry(stream, &dest.target(name), msg, opts, report, 0).await?;
    wire::write(stream, &Message::Done).await
}

async fn entry(
    stream: &mut impl Duplex,
    path: &Path,
    msg: Message,
    opts: Options,
    report: &mut Report<'_>,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "Directory nesting limit exceeded");
    let meta = paths::metadata(path).await?;
    ensure!(
        !meta.as_ref().is_some_and(paths::is_link),
        "Destination is a symbolic link or junction: {}",
        path.display()
    );
    match msg {
        Message::Directory { attrs, .. } => {
            ensure!(opts.recursive, "Copying a directory requires -r");
            match fs::create_dir(path).await {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    let m = fs::symlink_metadata(path).await?;
                    ensure!(
                        m.is_dir() && !paths::is_link(&m),
                        "Expected a directory: {}",
                        path.display()
                    );
                }
                Err(e) => return Err(e).with_context(|| format!("Create {}", path.display())),
            }
            loop {
                let child = wire::read(stream).await?;
                if matches!(child, Message::EndDirectory) {
                    break;
                }
                let target = path.join(entry_name(&child)?);
                Box::pin(entry(stream, &target, child, opts, report, depth + 1)).await?;
            }
            if opts.preserve
                && let Some(attrs) = attrs
            {
                attrs.apply(path)?;
            }
            Ok(())
        }
        Message::File { size, attrs, .. } => {
            ensure!(
                meta.as_ref().is_none_or(|m| m.is_file()),
                "Expected a regular file: {}",
                path.display()
            );
            if opts.no_clobber && meta.is_some() {
                report(Progress {
                    path,
                    bytes: 0,
                    size,
                    stage: Stage::Skipped,
                });
                return wire::write(stream, &Message::Skipped).await;
            }
            let stage = file(stream, path, size, attrs, meta, opts, report)
                .await
                .with_context(|| format!("Receive {}", path.display()))?;
            report(Progress {
                path,
                bytes: size,
                size,
                stage,
            });
            wire::write(
                stream,
                if stage == Stage::Skipped {
                    &Message::Skipped
                } else {
                    &Message::Done
                },
            )
            .await
        }
        _ => bail!("Expected a file or directory"),
    }
}

async fn file(
    stream: &mut impl Duplex,
    path: &Path,
    size: u64,
    attrs: Option<Attributes>,
    existing: Option<std::fs::Metadata>,
    opts: Options,
    report: &mut Report<'_>,
) -> Result<Stage> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".paxri-copy-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The OS applies the process umask. Never expose replacement content more broadly
        // than the existing file or an explicitly preserved source mode.
        let mode = if opts.preserve {
            attrs.as_ref().and_then(|a| a.mode)
        } else {
            None
        }
        .or_else(|| existing.as_ref().map(|m| m.permissions().mode()))
        .unwrap_or(0o666);
        builder.permissions(std::fs::Permissions::from_mode(mode & 0o777));
    }
    let (handle, temp) = builder.tempfile_in(parent)?.into_parts();
    let mut file = fs::File::from_std(handle);
    if opts.no_clobber {
        wire::write(stream, &Message::Ready).await?;
    }
    let mut bytes = 0;
    let mut buf = vec![0; wire::CHUNK];
    report(Progress {
        path,
        bytes,
        size,
        stage: Stage::Copying,
    });
    loop {
        let n = wire::read_data(stream, &mut buf).await?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        ensure!(bytes <= size, "Received more than the declared file size");
        file.write_all(&buf[..n]).await?;
        report(Progress {
            path,
            bytes,
            size,
            stage: Stage::Copying,
        });
    }
    ensure!(
        bytes == size,
        "Incomplete file: received {bytes} of {size} bytes"
    );
    file.flush().await?;
    drop(file.into_std().await);
    if opts.preserve
        && let Some(attrs) = attrs
    {
        attrs.apply(&temp)?;
    } else if let Some(meta) = existing {
        fs::set_permissions(&temp, meta.permissions()).await?;
    }
    if opts.no_clobber {
        match temp.persist_noclobber(path) {
            Ok(()) => {}
            Err(e) if e.error.kind() == ErrorKind::AlreadyExists => return Ok(Stage::Skipped),
            Err(e) => return Err(e.into()),
        }
    } else {
        temp.persist(path)?;
    }
    Ok(Stage::Copied)
}
