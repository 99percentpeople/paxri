//! Launch request for a command in a remote PTY. The session then uses ordinary terminal frames.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_REQUEST: usize = 32 * 1024;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Invocation {
    Program { program: String, args: Vec<String> },
    Shell { script: String },
}

impl Invocation {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Program { program, args } => {
                ensure!(!program.is_empty(), "missing remote program");
                ensure!(
                    !program.contains('\0') && args.iter().all(|a| !a.contains('\0')),
                    "NUL in remote program or argument"
                );
            }
            Self::Shell { script } => {
                ensure!(
                    !script.is_empty() && !script.contains('\0'),
                    "invalid remote shell command"
                );
            }
        }
        Ok(())
    }

    pub async fn write(&self, output: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
        self.validate()?;
        let data = serde_json::to_vec(self)?;
        ensure!(data.len() <= MAX_REQUEST, "remote command exceeds 32 KiB");
        output.write_u16(data.len() as u16).await?;
        output.write_all(&data).await?;
        Ok(())
    }

    pub async fn read(input: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        let size = usize::from(input.read_u16().await?);
        ensure!(
            (1..=MAX_REQUEST).contains(&size),
            "invalid remote command length"
        );
        let mut data = vec![0; size];
        input.read_exact(&mut data).await?;
        let invocation: Self = serde_json::from_slice(&data)?;
        invocation.validate()?;
        Ok(invocation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preserves_argument_boundaries_and_bounds_the_wire_request() {
        let command = Invocation::Program {
            program: "demo".into(),
            args: vec!["".into(), "a b".into(), "你\"好".into(), "-x".into()],
        };
        let mut bytes = Vec::new();
        command.write(&mut bytes).await.unwrap();
        assert_eq!(
            Invocation::read(&mut bytes.as_slice()).await.unwrap(),
            command
        );
        assert!(Invocation::read(&mut &[0x80, 1][..]).await.is_err());
        assert!(
            Invocation::Shell {
                script: "x".repeat(MAX_REQUEST)
            }
            .write(&mut Vec::new())
            .await
            .is_err()
        );
        assert!(
            Invocation::Shell {
                script: "a\0b".into()
            }
            .write(&mut Vec::new())
            .await
            .is_err()
        );
    }
}
