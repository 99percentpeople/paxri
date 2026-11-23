use anyhow::Result;

#[cfg(unix)]
pub(super) fn is_standard_user() -> Result<bool> {
    Ok(!rustix::process::geteuid().is_root())
}

#[cfg(windows)]
pub(super) fn is_standard_user() -> Result<bool> {
    use std::{
        mem,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut raw = std::ptr::null_mut();
    // Only a standard user token may be used for remote user services.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let token = OwnedHandle::from_raw_handle(raw);
        let mut elevation: TOKEN_ELEVATION = mem::zeroed();
        let mut length = 0;
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            mem::size_of_val(&elevation) as u32,
            &mut length,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(elevation.TokenIsElevated == 0)
    }
}
