//! Terminal framing on a single authenticated, ordered, bidirectional stream.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub mod exec;
pub mod managed;
mod open;
pub mod pipe;
pub mod pty;

pub use open::open;

pub const MAX_DATA: usize = 32 * 1024;
pub const READY: u8 = 0;
pub const DENIED: u8 = 1;
pub const UNAVAILABLE: u8 = 2;
pub const BUSY: u8 = 3;
pub const PRIVILEGED: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

impl Size {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            (1..=1024).contains(&self.rows) && (1..=1024).contains(&self.cols),
            "terminal dimensions must be between 1 and 1024"
        );
        Ok(self)
    }

    pub async fn read(input: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        Self {
            rows: input.read_u16().await?,
            cols: input.read_u16().await?,
        }
        .validate()
    }

    pub async fn write(self, output: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
        self.validate()?;
        output.write_u16(self.rows).await?;
        output.write_u16(self.cols).await?;
        Ok(())
    }
}

impl From<portable_pty::PtySize> for Size {
    fn from(size: portable_pty::PtySize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
        }
    }
}

impl From<Size> for portable_pty::PtySize {
    fn from(size: Size) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

pub enum Frame {
    Data(Vec<u8>),
    Resize(Size),
    Exit(u32),
}

impl Frame {
    pub async fn read(input: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        match input.read_u8().await? {
            1 => {
                let n = usize::from(input.read_u16().await?);
                ensure!((1..=MAX_DATA).contains(&n), "invalid terminal data length");
                let mut bytes = vec![0; n];
                input.read_exact(&mut bytes).await?;
                Ok(Self::Data(bytes))
            }
            2 => Ok(Self::Resize(Size::read(input).await?)),
            3 => Ok(Self::Exit(input.read_u32().await?)),
            _ => bail!("invalid terminal frame"),
        }
    }

    pub async fn write(&self, output: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
        match self {
            Self::Data(bytes) => {
                // A single paste may be larger than one frame.
                for chunk in bytes.chunks(MAX_DATA) {
                    output.write_u8(1).await?;
                    output.write_u16(chunk.len() as u16).await?;
                    output.write_all(chunk).await?;
                }
            }
            Self::Resize(size) => {
                output.write_u8(2).await?;
                size.write(output).await?;
            }
            Self::Exit(code) => {
                output.write_u8(3).await?;
                output.write_u32(*code).await?;
            }
        }
        output.flush().await?;
        Ok(())
    }
}

async fn send_input(
    mut writer: impl AsyncWrite + Unpin,
    input: async_channel::Receiver<Vec<u8>>,
    sizes: async_channel::Receiver<portable_pty::PtySize>,
    end: impl std::future::Future<Output = ()>,
) -> Result<()> {
    tokio::pin!(end);
    loop {
        let frame = tokio::select! {
            _ = &mut end => {
                Frame::Exit(0).write(&mut writer).await?;
                return std::future::pending().await;
            }
            data = input.recv() => match data { Ok(data) => Frame::Data(data), Err(_) => break },
            size = sizes.recv() => match size { Ok(size) => Frame::Resize(size.into()), Err(_) => break },
        };
        frame.write(&mut writer).await?;
    }
    Ok(())
}

pub async fn client(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    input: async_channel::Receiver<Vec<u8>>,
    sizes: async_channel::Receiver<portable_pty::PtySize>,
    output: &async_channel::Sender<pty::Output>,
) -> Result<u32> {
    run_client(stream, input, sizes, output, std::future::pending()).await
}

async fn run_client(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    input: async_channel::Receiver<Vec<u8>>,
    sizes: async_channel::Receiver<portable_pty::PtySize>,
    output: &async_channel::Sender<pty::Output>,
    end: impl std::future::Future<Output = ()>,
) -> Result<u32> {
    let (mut reader, writer) = tokio::io::split(stream);
    let receive = async {
        loop {
            match Frame::read(&mut reader)
                .await
                .context("remote terminal disconnected")?
            {
                Frame::Data(data) => output.send(pty::Output::Bytes(data)).await?,
                Frame::Exit(code) => return Ok(code),
                Frame::Resize(_) => bail!("unexpected resize from terminal host"),
            }
        }
    };
    tokio::select! {
        result = receive => result,
        result = send_input(writer, input, sizes, end) => { result?; bail!("terminal closed") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paste_is_split_into_bounded_frames_and_ends_before_exit() {
        let bytes = vec![0xa5; MAX_DATA + 9];
        let mut stream = Vec::new();
        Frame::Data(bytes.clone()).write(&mut stream).await.unwrap();
        Frame::Exit(23).write(&mut stream).await.unwrap();
        let mut reader = stream.as_slice();
        for expected in bytes.chunks(MAX_DATA) {
            let Frame::Data(data) = Frame::read(&mut reader).await.unwrap() else {
                panic!("expected data")
            };
            assert_eq!(data, expected);
        }
        assert!(matches!(
            Frame::read(&mut reader).await.unwrap(),
            Frame::Exit(23)
        ));
        assert!(reader.is_empty());
    }

    #[tokio::test]
    async fn invalid_sizes_and_frame_lengths_are_rejected() {
        for bytes in [
            &[1, 0, 0][..],
            &[1, 128, 1],
            &[2, 0, 0, 0, 80],
            &[2, 0, 25, 4, 1],
            &[42],
            &[1, 0, 2, 65],
        ] {
            assert!(Frame::read(&mut &bytes[..]).await.is_err());
        }
    }
}
