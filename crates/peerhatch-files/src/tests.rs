use super::*;
use tempfile::tempdir;
use tokio::io::duplex;
use wire::Message::*;

async fn start_upload(
    path: &Path,
) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<Result<()>>) {
    let (mut client, server) = duplex(512);
    let task = tokio::spawn(serve(server, true));
    let req = Put {
        path: path.to_str().unwrap().into(),
        options: Options {
            recursive: true,
            ..Default::default()
        },
        require_directory: false,
    };
    wire::write(&mut client, &req).await.unwrap();
    assert!(matches!(wire::read(&mut client).await.unwrap(), Ready));
    (client, task)
}

#[tokio::test]
async fn interrupted_or_short_file_never_replaces_previous_contents() -> Result<()> {
    let dir = tempdir()?;
    let dest = dir.path().join("kept");
    std::fs::write(&dest, "original")?;
    for explicit_end in [false, true] {
        let (mut stream, server) = start_upload(&dest).await;
        wire::write(
            &mut stream,
            &File {
                name: "incoming".into(),
                size: 9,
                attrs: None,
            },
        )
        .await?;
        wire::write_data(&mut stream, b"short").await?;
        if explicit_end {
            wire::write(&mut stream, &EndFile).await?;
            assert!(
                format!("{:#}", wire::read(&mut stream).await.unwrap_err())
                    .contains("Incomplete file")
            );
        }
        drop(stream);
        assert!(server.await?.is_err());
        assert_eq!(std::fs::read(&dest)?, b"original");
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn reject_directory_escape_and_unrequested_download_name() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("target");
    std::fs::create_dir(&output)?;
    for name in ["../outside", "..", "dir/child", r"..\outside", ""] {
        let (mut stream, server) = start_upload(&output).await;
        wire::write(
            &mut stream,
            &Directory {
                name: "tree".into(),
                attrs: None,
            },
        )
        .await?;
        wire::write(
            &mut stream,
            &Directory {
                name: name.into(),
                attrs: None,
            },
        )
        .await?;
        assert!(wire::read(&mut stream).await.is_err());
        assert!(server.await?.is_err());
        assert!(!dir.path().join("outside").exists());
    }
    let (a, mut b) = duplex(1024);
    let mut ignore = |_: Progress| {};
    let (client, peer) = tokio::join!(
        copy(
            a,
            Copy::Download {
                source: "notes",
                destination: &output
            },
            Options::default(),
            false,
            &mut ignore
        ),
        async {
            let _ = wire::read(&mut b).await?;
            wire::write(&mut b, &Ready).await?;
            wire::write(
                &mut b,
                &File {
                    name: "other".into(),
                    size: 0,
                    attrs: None,
                },
            )
            .await
        }
    );
    peer?;
    assert!(
        client
            .unwrap_err()
            .to_string()
            .contains("unexpected file name")
    );
    assert!(!output.join("other").exists());
    Ok(())
}

#[tokio::test]
async fn no_clobber_handles_competing_destinations_at_commit() -> Result<()> {
    let dir = tempdir()?;
    let dest = dir.path().join("race");
    let (mut client, server) = duplex(512);
    let server = tokio::spawn(serve(server, true));
    wire::write(
        &mut client,
        &Put {
            path: dest.to_str().unwrap().into(),
            options: Options {
                no_clobber: true,
                ..Default::default()
            },
            require_directory: false,
        },
    )
    .await?;
    assert!(matches!(wire::read(&mut client).await?, Ready));
    wire::write(
        &mut client,
        &File {
            name: "race".into(),
            size: 3,
            attrs: None,
        },
    )
    .await?;
    assert!(matches!(wire::read(&mut client).await?, Ready));
    std::fs::write(&dest, "other writer")?;
    wire::write_data(&mut client, b"new").await?;
    wire::write(&mut client, &EndFile).await?;
    assert!(matches!(wire::read(&mut client).await?, Skipped));
    assert!(matches!(wire::read(&mut client).await?, Done));
    server.await??;
    assert_eq!(std::fs::read(dest)?, b"other writer");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn elevated_host_declines_before_accessing_files() -> Result<()> {
    let dir = tempdir()?;
    let dest = dir.path().join("denied");
    let (a, b) = duplex(512);
    let mut ignore = |_: Progress| {};
    let (result, _) = tokio::join!(
        copy(
            a,
            Copy::Upload {
                source: &dest,
                destination: dest.to_str().unwrap()
            },
            Options::default(),
            false,
            &mut ignore
        ),
        serve(b, false)
    );
    assert!(format!("{:#}", result.unwrap_err()).contains("non-elevated"));
    assert!(!dest.exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn reject_existing_destination_link_during_recursion() -> Result<()> {
    let dir = tempdir()?;
    let dest = dir.path().join("dest");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(dest.join("tree"))?;
    std::fs::create_dir(&outside)?;
    std::os::unix::fs::symlink(&outside, dest.join("tree/link"))?;
    let (mut stream, task) = start_upload(&dest).await;
    for name in ["tree", "link"] {
        wire::write(
            &mut stream,
            &Directory {
                name: name.into(),
                attrs: None,
            },
        )
        .await?;
    }
    assert!(wire::read(&mut stream).await.is_err());
    assert!(task.await?.is_err());
    assert_eq!(std::fs::read_dir(outside)?.count(), 0);
    Ok(())
}
