use paxri_terminal::pty::CommandBuilder;
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct SessionTools {
    ipc: PathBuf,
    companion: Option<PathBuf>,
    extensions: PathBuf,
}

impl SessionTools {
    pub(crate) fn new(ipc: PathBuf, profile_dir: &Path) -> Self {
        let companion = env::current_exe()
            .ok()
            .map(|exe| exe.with_file_name(format!("paxri{}", env::consts::EXE_SUFFIX)));
        Self {
            ipc,
            companion,
            extensions: profile_dir.join("tools").join("bin"),
        }
    }

    pub(super) fn apply(&self, cmd: &mut CommandBuilder, caller_id: &str) {
        let companion = self.companion.as_deref().filter(|p| p.is_file());
        let dirs = [
            companion.and_then(Path::parent),
            self.extensions
                .is_dir()
                .then_some(self.extensions.as_path()),
        ];
        // A directory containing the platform's PATH separator cannot be added.
        let dirs = dirs
            .into_iter()
            .flatten()
            .filter(|p| env::join_paths([p]).is_ok());
        if let Ok(prefix) = env::join_paths(dirs)
            && !prefix.is_empty()
        {
            prepend_tool_path(cmd, &prefix);
        }
        if let Some(cli) = companion
            && let Some(program) = cmd.get_argv_mut().first_mut()
            && is_cli_name(program)
        {
            *program = cli.as_os_str().to_owned();
        }
        cmd.env("PAXRI_IPC", &self.ipc);
        cmd.env("PAXRI_CALLER_ID", caller_id);
        // Installed session tools may still read the original variable names.
        cmd.env("PEERHATCH_IPC", &self.ipc);
        cmd.env("PEERHATCH_CALLER_ID", caller_id);
    }
}

fn is_cli_name(name: &OsStr) -> bool {
    #[cfg(windows)]
    return name.eq_ignore_ascii_case("paxri") || name.eq_ignore_ascii_case("paxri.exe");
    #[cfg(unix)]
    return name == "paxri";
}

fn prepend_tool_path(cmd: &mut CommandBuilder, prefix: &OsStr) {
    #[cfg(unix)]
    if cmd.is_default_prog() {
        let shell = cmd.get_shell();
        if matches!(
            Path::new(&shell).file_name().and_then(OsStr::to_str),
            Some("sh" | "ash" | "dash" | "bash" | "ksh" | "zsh")
        ) {
            // Login profiles can replace PATH entirely (e.g. Debian /etc/profile).
            // Read them first, then exec the interactive shell with its normal RC.
            *cmd = CommandBuilder::new(&shell);
            cmd.args([
                "-lc",
                r#"export PATH="$1${PATH:+:$PATH}"; exec "$0" -i"#,
                &shell,
            ]);
            cmd.arg(prefix);
            return;
        }
    }
    let mut path = OsString::from(prefix);
    if let Some(inherited) = cmd.get_env("PATH").filter(|p| !p.is_empty()) {
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(inherited);
    }
    cmd.env("PATH", path);
}
