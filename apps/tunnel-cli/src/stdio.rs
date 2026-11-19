//! Own the output pipe so a remote FIN delivers EOF even while stdin is still open.
use std::{fs::File, io};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
pub fn input() -> io::Result<impl AsyncRead + Unpin> {
    Ok(tokio::io::stdin())
}

#[cfg(unix)]
pub fn output() -> io::Result<impl AsyncWrite + Unpin> {
    use std::os::fd::AsFd;
    let output = io::stdout().as_fd().try_clone_to_owned()?;
    rustix::stdio::dup2_stdout(File::options().write(true).open("/dev/null")?)?;
    Ok(tokio::fs::File::from_std(output.into()))
}

#[cfg(windows)]
pub fn output() -> io::Result<Box<dyn AsyncWrite + Unpin>> {
    use std::os::windows::io::AsHandle;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        },
    };
    let output = io::stdout().as_handle().try_clone_to_owned()?;
    // This CLI command exclusively owns stdout. Keep any shared stdin/stderr handle live.
    unsafe {
        let original = GetStdHandle(STD_OUTPUT_HANDLE);
        if SetStdHandle(STD_OUTPUT_HANDLE, std::ptr::null_mut()) == 0 {
            return Err(io::Error::last_os_error());
        }
        if original != GetStdHandle(STD_ERROR_HANDLE) && original != GetStdHandle(STD_INPUT_HANDLE)
        {
            CloseHandle(original);
        }
    }
    if overlapped_pipe(&output)? {
        Ok(Box::new(async_pipe(output)?))
    } else {
        Ok(Box::new(tokio::fs::File::from_std(File::from(output))))
    }
}

#[cfg(windows)]
pub fn input() -> io::Result<Box<dyn AsyncRead + Unpin>> {
    use std::os::windows::io::AsHandle;
    let input = io::stdin().as_handle().try_clone_to_owned()?;
    if overlapped_pipe(&input)? {
        Ok(Box::new(async_pipe(input)?))
    } else {
        Ok(Box::new(tokio::io::stdin()))
    }
}

#[cfg(windows)]
fn overlapped_pipe(handle: &std::os::windows::io::OwnedHandle) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::{
        Wdk::Storage::FileSystem::{
            FILE_MODE_INFORMATION, FILE_SYNCHRONOUS_IO_ALERT, FILE_SYNCHRONOUS_IO_NONALERT,
            FileModeInformation, NtQueryInformationFile,
        },
        Win32::{
            Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType},
            System::IO::IO_STATUS_BLOCK,
        },
    };
    let raw = handle.as_raw_handle();
    // Win32 OpenSSH creates overlapped proxy pipes. Ordinary shell pipes are synchronous.
    if unsafe { GetFileType(raw) } != FILE_TYPE_PIPE {
        return Ok(false);
    }
    let mut status = IO_STATUS_BLOCK::default();
    let mut mode = FILE_MODE_INFORMATION::default();
    // FileModeInformation is a synchronous query of the file object's open flags.
    let result = unsafe {
        NtQueryInformationFile(
            raw,
            &mut status,
            (&raw mut mode).cast(),
            size_of_val(&mode) as u32,
            FileModeInformation,
        )
    };
    if result != 0 {
        return Err(io::Error::other(format!(
            "query standard pipe mode: NTSTATUS {result:#x}"
        )));
    }
    Ok(mode.Mode & (FILE_SYNCHRONOUS_IO_ALERT | FILE_SYNCHRONOUS_IO_NONALERT) == 0)
}

#[cfg(windows)]
fn async_pipe(
    handle: std::os::windows::io::OwnedHandle,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use std::os::windows::io::IntoRawHandle;
    // Ownership is transferred to Tokio while the runtime is active.
    unsafe {
        tokio::net::windows::named_pipe::NamedPipeClient::from_raw_handle(handle.into_raw_handle())
    }
}
