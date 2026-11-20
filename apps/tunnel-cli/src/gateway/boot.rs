//! Windows may give a batch logon an elevated token even for a LeastPrivilege task.
//! The boot task invokes a protected copy of this program. Only its standard-user
//! child opens the portable binaries, node files, logs, or IPC endpoint.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    io, mem,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    Security::{
        CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, GetTokenInformation,
        LUA_TOKEN, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, SetTokenInformation,
        TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel, WinMediumLabelSid,
    },
    System::{
        SystemServices::SE_GROUP_INTEGRITY,
        Threading::{
            CreateProcessAsUserW, DETACHED_PROCESS, GetCurrentProcess, GetExitCodeProcess,
            INFINITE, OpenProcessToken, PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
        },
    },
};

fn checked(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn set_user_default_dacl(token: &OwnedHandle) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::GENERIC_ALL,
        Security::{
            ACL, ACL_REVISION, AddAccessAllowedAce, InitializeAcl, TOKEN_DEFAULT_DACL, TOKEN_USER,
            TokenDefaultDacl, TokenUser, WinLocalSystemSid,
        },
    };
    // A batch administrator's default DACL can grant access only to Administrators and SYSTEM.
    // Once Administrators becomes deny-only, even opening the second end of CreatePipe fails.
    let mut user = [0usize;
        (size_of::<TOKEN_USER>() + SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<usize>())];
    let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
    let mut bytes = mem::size_of_val(&system) as u32;
    // Two ACEs, each with room for a maximum-length SID.
    let mut acl_storage = [0u32; 48];
    let acl = acl_storage.as_mut_ptr().cast::<ACL>();
    unsafe {
        checked(CreateWellKnownSid(
            WinLocalSystemSid,
            null_mut(),
            system.as_mut_ptr().cast(),
            &mut bytes,
        ))?;
        checked(GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            mem::size_of_val(&user) as u32,
            &mut bytes,
        ))?;
        checked(InitializeAcl(
            acl,
            mem::size_of_val(&acl_storage) as u32,
            ACL_REVISION,
        ))?;
        for sid in [
            (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            system.as_mut_ptr().cast(),
        ] {
            checked(AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_ALL, sid))?;
        }
        let dacl = TOKEN_DEFAULT_DACL { DefaultDacl: acl };
        checked(SetTokenInformation(
            token.as_raw_handle(),
            TokenDefaultDacl,
            (&raw const dacl).cast(),
            size_of::<TOKEN_DEFAULT_DACL>() as u32,
        ))?;
    }
    Ok(())
}

fn current_token(access: u32) -> io::Result<OwnedHandle> {
    let mut handle = null_mut();
    unsafe {
        checked(OpenProcessToken(GetCurrentProcess(), access, &mut handle))?;
        Ok(OwnedHandle::from_raw_handle(handle))
    }
}

pub(super) fn user_token() -> Result<Option<OwnedHandle>> {
    let current = current_token(TOKEN_QUERY)?;
    let mut restricted = null_mut();
    // The Win32 calls below return newly owned handles. All passed buffers live until the call returns.
    unsafe {
        let mut length = 0;
        let mut elevation = TOKEN_ELEVATION::default();
        checked(GetTokenInformation(
            current.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        ))?;
        if elevation.TokenIsElevated == 0 {
            use windows_sys::Win32::{
                Security::{GetSidSubAuthority, GetSidSubAuthorityCount},
                System::SystemServices::SECURITY_MANDATORY_MEDIUM_RID,
            };
            let mut buffer = [0usize;
                (size_of::<TOKEN_MANDATORY_LABEL>() + SECURITY_MAX_SID_SIZE as usize)
                    .div_ceil(size_of::<usize>())];
            checked(GetTokenInformation(
                current.as_raw_handle(),
                TokenIntegrityLevel,
                buffer.as_mut_ptr().cast(),
                mem::size_of_val(&buffer) as u32,
                &mut length,
            ))?;
            let sid = (*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
            let last = u32::from(*GetSidSubAuthorityCount(sid))
                .checked_sub(1)
                .context("Invalid integrity SID")?;
            if *GetSidSubAuthority(sid, last) <= SECURITY_MANDATORY_MEDIUM_RID as u32 {
                return Ok(None);
            }
        }
        let current = current_token(
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
        )?;
        checked(CreateRestrictedToken(
            current.as_raw_handle(),
            LUA_TOKEN | DISABLE_MAX_PRIVILEGE,
            0,
            null(),
            0,
            null(),
            0,
            null(),
            &mut restricted,
        ))
        .context("Create the standard-user boot token")?;
        let token = OwnedHandle::from_raw_handle(restricted);
        let mut medium = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
        let mut size = mem::size_of_val(&medium) as u32;
        checked(CreateWellKnownSid(
            WinMediumLabelSid,
            null_mut(),
            medium.as_mut_ptr().cast(),
            &mut size,
        ))?;
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: medium.as_mut_ptr().cast(),
                Attributes: SE_GROUP_INTEGRITY as u32,
            },
        };
        checked(SetTokenInformation(
            token.as_raw_handle(),
            TokenIntegrityLevel,
            (&raw const label).cast(),
            size_of::<TOKEN_MANDATORY_LABEL>() as u32 + size,
        ))
        .context("Set medium integrity for the boot token")?;
        let mut elevation = TOKEN_ELEVATION::default();
        checked(GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        ))?;
        ensure!(
            elevation.TokenIsElevated == 0,
            "Windows did not provide a standard-user boot token"
        );
        set_user_default_dacl(&token)
            .context("Set the standard user’s default object permissions")?;
        Ok(Some(token))
    }
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

pub(crate) fn start(client: &Path, config: &Path, ipc: &Path) -> Result<u32> {
    run_client(user_token()?, client, ipc, "start", Some(config))
}

pub(super) fn run_client(
    token: Option<OwnedHandle>,
    client: &Path,
    ipc: &Path,
    action: &str,
    config: Option<&Path>,
) -> Result<u32> {
    ensure!(
        client.is_absolute() && config.is_none_or(Path::is_absolute),
        "Gateway launcher paths must be absolute"
    );
    let mut args = vec![
        client,
        Path::new("--ipc"),
        ipc,
        Path::new("gateway"),
        Path::new(action),
    ];
    if let Some(path) = config {
        args.extend([Path::new("--config"), path]);
    }
    let mut command_line = Vec::new();
    for arg in &args {
        let units: Vec<_> = arg.as_os_str().encode_wide().collect();
        ensure!(
            !units.contains(&0) && !units.contains(&34) && units.last() != Some(&92),
            "Invalid boot argument"
        );
        command_line.push(34);
        command_line.extend(units);
        command_line.extend([34, 32]);
    }
    command_line.push(0);
    let exe = wide(client.as_os_str());
    let Some(token) = token else {
        use std::{
            os::windows::process::CommandExt,
            process::{Command, Stdio},
        };
        let status = Command::new(client)
            .args(&args[1..])
            .creation_flags(DETACHED_PROCESS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        return Ok(status
            .code()
            .context("Gateway launcher returned no exit status")? as u32);
    };
    let mut startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut child = PROCESS_INFORMATION::default();
    // Supplying the full application path prevents executable-name search. Inherit no handles.
    unsafe {
        checked(CreateProcessAsUserW(
            token.as_raw_handle(),
            exe.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            0,
            DETACHED_PROCESS,
            null(),
            null(),
            &mut startup,
            &mut child,
        ))
        .context("Launch the gateway client as a standard user")?;
        let process = OwnedHandle::from_raw_handle(child.hProcess);
        drop(OwnedHandle::from_raw_handle(child.hThread));
        ensure!(
            WaitForSingleObject(process.as_raw_handle(), INFINITE) == WAIT_OBJECT_0,
            "Wait for the gateway launcher"
        );
        let mut code = 0;
        checked(GetExitCodeProcess(process.as_raw_handle(), &mut code))?;
        Ok(code)
    }
}
