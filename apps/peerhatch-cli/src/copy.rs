use anyhow::{Result, bail};
use clap::Args;
use std::{
    io::{IsTerminal, Write},
    path::Path,
    time::{Duration, Instant},
};
use peerhatch_control::files::{Copy, Options, Progress, Stage};

#[derive(Args)]
#[command(
    after_help = "Examples\n  peerhatch cp build.zip home:Downloads/\n  peerhatch cp -rp home:projects/demo ./backups/\n  peerhatch cp -nv one.txt two.txt home:Documents/"
)]
pub struct CopyArgs {
    /// Copy directories, including hidden files and empty directories
    #[arg(short = 'r', long)]
    recursive: bool,
    /// Preserve timestamps and Unix permission bits where supported
    #[arg(short = 'p', long)]
    preserve: bool,
    /// Skip existing files
    #[arg(short = 'n', long)]
    no_clobber: bool,
    /// Suppress progress and file listings
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    quiet: bool,
    /// List each copied or skipped file on stderr
    #[arg(short = 'v', long)]
    verbose: bool,
    /// Local paths or PEER:PATH, where PEER is a device name, full ID or unique ID prefix
    #[arg(required = true, num_args = 1.., value_name = "SOURCE")]
    sources: Vec<String>,
    /// File name or directory on the receiving device
    #[arg(value_name = "DESTINATION")]
    destination: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Operand<'a> {
    Local(&'a str),
    Remote(&'a str, &'a str),
}

impl<'a> Operand<'a> {
    fn parse(s: &'a str) -> Self {
        if let Some((prefix, path)) = s.split_once(':')
            && !prefix.is_empty()
            && !prefix.contains(['/', '\\'])
            && !(cfg!(windows)
                && prefix.len() == 1
                && prefix.as_bytes()[0].is_ascii_alphabetic()
                && path.starts_with(['/', '\\']))
        {
            Self::Remote(prefix, path)
        } else {
            Self::Local(s)
        }
    }
}

impl CopyArgs {
    pub async fn run(self, ipc: &Path) -> Result<u32> {
        let sources = &self.sources;
        let dest = Operand::parse(&self.destination);
        let opts = Options {
            recursive: self.recursive,
            no_clobber: self.no_clobber,
            preserve: self.preserve,
        };
        let mut planned = Vec::with_capacity(sources.len());
        for source in sources {
            match (Operand::parse(source), dest) {
                (Operand::Local(source), Operand::Remote(peer, destination)) => planned.push((
                    peer,
                    Copy::Upload {
                        source: Path::new(source),
                        destination,
                    },
                )),
                (Operand::Remote(peer, source), Operand::Local(destination)) => planned.push((
                    peer,
                    Copy::Download {
                        source,
                        destination: Path::new(destination),
                    },
                )),
                _ => bail!("Each copy needs one local path and one PEER:PATH"),
            }
        }
        let require_directory = sources.len() > 1;
        let interactive = !self.quiet && std::io::stderr().is_terminal();
        let mut last_update: Option<Instant> = None;
        let mut width: usize = 0;
        let mut report = |p: Progress<'_>| {
            if interactive
                && (p.stage != Stage::Copying
                    || last_update.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)))
            {
                let label = p.label();
                let len = label.chars().count();
                eprint!("\r{label}{}", " ".repeat(width.saturating_sub(len)));
                let _ = std::io::stderr().flush();
                if p.stage == Stage::Copying {
                    width = len;
                    last_update = Some(Instant::now());
                } else {
                    eprintln!();
                    width = 0;
                    last_update = None;
                }
            } else if !interactive && self.verbose && p.stage != Stage::Copying {
                eprintln!("{}", p.label());
            }
        };
        let run = async {
            for (peer, copy) in planned {
                peerhatch_control::copy_files(ipc, peer, copy, opts, require_directory, &mut report)
                    .await?;
            }
            Ok(0)
        };
        let result = tokio::select! {
            result = run => result,
            signal = tokio::signal::ctrl_c() => { signal?; Ok(130) }
        };
        if width > 0 {
            eprintln!();
        }
        result
    }
}
