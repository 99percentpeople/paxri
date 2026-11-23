use super::{
    Duplex, MAX_DEPTH, Options, Progress, Report, Stage, paths,
    wire::{self, Attributes, Message},
};
use anyhow::{Context, Result, ensure};
use std::path::Path;
use tokio::{fs, io::AsyncReadExt};

pub async fn tree(
    stream: &mut impl Duplex,
    path: &Path,
    opts: Options,
    report: &mut Report<'_>,
) -> Result<()> {
    // "." and ".." have no final component. Give their resolved directory its real name.
    let resolved;
    let name = if path.file_name().is_some() {
        paths::name(path)?
    } else {
        resolved = fs::canonicalize(path).await?;
        paths::name(&resolved)?
    };
    entry(stream, path, name, opts, report, 0).await?;
    ensure!(
        matches!(wire::read(stream).await?, Message::Done),
        "Missing final copy confirmation"
    );
    Ok(())
}

async fn entry(
    stream: &mut impl Duplex,
    path: &Path,
    name: &str,
    opts: Options,
    report: &mut Report<'_>,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "Directory nesting limit exceeded");
    paths::validate_name(name)?;
    let meta = fs::symlink_metadata(path)
        .await
        .with_context(|| format!("Read {}", path.display()))?;
    ensure!(
        !paths::is_link(&meta),
        "Cannot copy a symbolic link or junction: {}",
        path.display()
    );
    let attrs = opts
        .preserve
        .then(|| Attributes::from_metadata(&meta))
        .transpose()?;
    if meta.is_dir() {
        ensure!(opts.recursive, "{} is a directory (use -r)", path.display());
        let mut children = fs::read_dir(path)
            .await
            .with_context(|| format!("Read directory {}", path.display()))?;
        wire::write(
            stream,
            &Message::Directory {
                name: name.into(),
                attrs,
            },
        )
        .await?;
        while let Some(child) = children.next_entry().await? {
            let child = child.path();
            Box::pin(entry(
                stream,
                &child,
                paths::name(&child)?,
                opts,
                report,
                depth + 1,
            ))
            .await?;
        }
        return wire::write(stream, &Message::EndDirectory).await;
    }
    ensure!(
        meta.is_file(),
        "Expected a regular file: {}",
        path.display()
    );
    let mut file = fs::File::open(path)
        .await
        .with_context(|| format!("Open {}", path.display()))?;
    let size = file.metadata().await?.len();
    wire::write(
        stream,
        &Message::File {
            name: name.into(),
            size,
            attrs,
        },
    )
    .await?;
    if opts.no_clobber {
        match wire::read(stream).await? {
            Message::Skipped => {
                report(Progress {
                    path,
                    bytes: 0,
                    size,
                    stage: Stage::Skipped,
                });
                return Ok(());
            }
            Message::Ready => {}
            _ => anyhow::bail!("Invalid no-clobber response"),
        }
    }
    let (mut rx, mut tx) = tokio::io::split(stream);
    // A receiver that runs out of disk space can interrupt a writer blocked on network capacity.
    let send = async {
        let mut bytes = 0;
        let mut buf = vec![0u8; wire::CHUNK];
        report(Progress {
            path,
            bytes,
            size,
            stage: Stage::Copying,
        });
        loop {
            let count = file.read(&mut buf).await?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            ensure!(bytes <= size, "File grew during copy: {}", path.display());
            wire::write_data(&mut tx, &buf[..count]).await?;
            report(Progress {
                path,
                bytes,
                size,
                stage: Stage::Copying,
            });
        }
        ensure!(bytes == size, "File shrank during copy: {}", path.display());
        wire::write(&mut tx, &Message::EndFile).await
    };
    let confirm = async {
        match wire::read(&mut rx).await? {
            Message::Done => Ok(Stage::Copied),
            Message::Skipped if opts.no_clobber => Ok(Stage::Skipped),
            _ => anyhow::bail!("Invalid file confirmation"),
        }
    };
    let ((), stage) = tokio::try_join!(send, confirm)?;
    report(Progress {
        path,
        bytes: size,
        size,
        stage,
    });
    Ok(())
}
