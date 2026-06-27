// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use clap::{Arg, ArgAction, Command};
use std::io;
use std::ptr;
use uucore::error::{UResult, USimpleError};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_INSUFFICIENT_BUFFER, HANDLE,
};
use windows_sys::Win32::Security::{
    DuplicateTokenEx, EqualSid, GetTokenInformation, LookupAccountNameW, LookupAccountSidW,
    SecurityImpersonation, SetTokenInformation, TokenGroups, TokenPrimary, TokenPrimaryGroup,
    SID_NAME_USE, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
    TOKEN_GROUPS, TOKEN_PRIMARY_GROUP, TOKEN_QUERY,
};
use windows_sys::Win32::System::SystemServices::SE_GROUP_ENABLED;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    WaitForSingleObject, INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

pub fn uu_app() -> Command {
    Command::new("newgrp")
        .about("Log in to a new group")
        .arg(
            Arg::new("group")
                .help("The group to log into")
                .index(1)
                .required(false),
        )
        .arg(
            Arg::new("command")
                .short('c')
                .long("command")
                .help("Command to execute with /bin/sh")
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("list")
                .short('L')
                .long("list")
                .help("List available groups from the current token")
                .action(ArgAction::SetTrue),
        )
}

#[uucore::main(no_signals)]
pub fn uumain(args: impl uucore::Args) -> UResult<()> {
    let matches = uu_app().get_matches_from(args);
    let group = matches.get_one::<String>("group");
    let command = matches.get_one::<String>("command");
    let list = matches.get_flag("list");

    // Open the current process token
    let mut token: HANDLE = ptr::null_mut();
    unsafe {
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut token,
        ) == 0
        {
            return Err(USimpleError::new(
                1,
                format!(
                    "cannot open process token: {}",
                    io::Error::from_raw_os_error(GetLastError() as i32)
                ),
            ));
        }
    }

    // -L: list groups and exit
    if list {
        let result = list_token_groups(token);
        unsafe { CloseHandle(token) };
        return result;
    }

    let group = match group {
        Some(g) => g,
        None => {
            unsafe { CloseHandle(token) };
            return Err(USimpleError::new(1, "no group name given"));
        }
    };

    // Resolve group name to SID
    let sid = match lookup_account_name(group) {
        Ok(s) => s,
        Err(e) => {
            unsafe { CloseHandle(token) };
            return Err(USimpleError::new(
                1,
                format!("could not find group '{}': {}", group, e),
            ));
        }
    };

    // Security: verify the group is in TOKEN_GROUPS with SE_GROUP_ENABLED
    match is_group_in_token(token, &sid) {
        Ok(true) => {}
        Ok(false) => {
            unsafe { CloseHandle(token) };
            return Err(USimpleError::new(
                1,
                format!("current user is not a member of group '{}'", group),
            ));
        }
        Err(e) => {
            unsafe { CloseHandle(token) };
            return Err(USimpleError::new(
                1,
                format!("failed to query token groups: {}", e),
            ));
        }
    }

    // Duplicate the token so we don't alter our own process token
    let mut new_token: HANDLE = ptr::null_mut();
    unsafe {
        if DuplicateTokenEx(
            token,
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            ptr::null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut new_token,
        ) == 0
        {
            let err = GetLastError();
            CloseHandle(token);
            return Err(USimpleError::new(
                1,
                format!(
                    "DuplicateTokenEx failed: {}",
                    io::Error::from_raw_os_error(err as i32)
                ),
            ));
        }
        CloseHandle(token);
    }

    // Set the primary group on the duplicated token
    if let Err(e) = set_token_primary_group(new_token, &sid) {
        unsafe { CloseHandle(new_token) };
        return Err(USimpleError::new(
            1,
            format!("could not switch to new primary group '{}': {}", group, e),
        ));
    }

    // Build command line: if -c given, wrap in cmd.exe /c; otherwise open interactive shell
    let cmd = if let Some(c) = command {
        format!("cmd.exe /c {}", c)
    } else {
        "cmd.exe".to_string()
    };

    let exit_code = spawn_process_as_user(new_token, &cmd).map_err(|e| {
        unsafe { CloseHandle(new_token) };
        USimpleError::new(
            1,
            format!("failed to execute '{}': {}", cmd, e),
        )
    })?;

    unsafe { CloseHandle(new_token) };

    if exit_code != 0 {
        std::process::exit(exit_code as i32);
    }

    Ok(())
}

/// Resolves an account name to a raw SID buffer.
fn lookup_account_name(name: &str) -> io::Result<Vec<u8>> {
    unsafe {
        let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sid_size: u32 = 0;
        let mut domain_size: u32 = 0;
        let mut pe_use: SID_NAME_USE = 0;

        // First call: get required buffer sizes
        LookupAccountNameW(
            ptr::null(),
            name_w.as_ptr(),
            ptr::null_mut(),
            &mut sid_size,
            ptr::null_mut(),
            &mut domain_size,
            &mut pe_use,
        );

        if GetLastError() != ERROR_INSUFFICIENT_BUFFER {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        let mut sid = vec![0u8; sid_size as usize];
        let mut domain = vec![0u16; domain_size as usize];

        if LookupAccountNameW(
            ptr::null(),
            name_w.as_ptr(),
            sid.as_mut_ptr() as _,
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut pe_use,
        ) == 0
        {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        Ok(sid)
    }
}

/// Returns true if the given SID is present in TOKEN_GROUPS with SE_GROUP_ENABLED.
fn is_group_in_token(token: HANDLE, sid: &[u8]) -> io::Result<bool> {
    let groups = get_token_groups(token)?;

    unsafe {
        let ptgroups = groups.as_ptr() as *const TOKEN_GROUPS;
        let count = (*ptgroups).GroupCount as usize;
        // SAFETY: TOKEN_GROUPS is followed by GroupCount SID_AND_ATTRIBUTES entries.
        let groups_slice = std::slice::from_raw_parts((*ptgroups).Groups.as_ptr(), count);

        for entry in groups_slice {
            if entry.Attributes & (SE_GROUP_ENABLED as u32) != 0
                && EqualSid(sid.as_ptr() as _, entry.Sid) != 0
            {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Lists all groups in the token that have the SE_GROUP_ENABLED flag.
fn list_token_groups(token: HANDLE) -> UResult<()> {
    let groups = get_token_groups(token).map_err(|e| {
        USimpleError::new(1, format!("failed to query token groups: {}", e))
    })?;

    unsafe {
        let ptgroups = groups.as_ptr() as *const TOKEN_GROUPS;
        let count = (*ptgroups).GroupCount as usize;
        let groups_slice = std::slice::from_raw_parts((*ptgroups).Groups.as_ptr(), count);

        for entry in groups_slice {
            if entry.Attributes & (SE_GROUP_ENABLED as u32) == 0 {
                continue;
            }

            let mut name_size: u32 = 256;
            let mut name_buf = vec![0u16; name_size as usize];
            let mut domain_size: u32 = 256;
            let mut domain_buf = vec![0u16; domain_size as usize];
            let mut pe_use: SID_NAME_USE = 0;

            if LookupAccountSidW(
                ptr::null(),
                entry.Sid,
                name_buf.as_mut_ptr(),
                &mut name_size,
                domain_buf.as_mut_ptr(),
                &mut domain_size,
                &mut pe_use,
            ) == 0
            {
                // Skip entries we can't resolve
                continue;
            }

            let name = String::from_utf16_lossy(&name_buf[..name_size as usize]);
            println!("{}", name);
        }
    }

    Ok(())
}

/// Retrieves TOKEN_GROUPS from the token into a raw byte buffer.
fn get_token_groups(token: HANDLE) -> io::Result<Vec<u8>> {
    unsafe {
        let mut needed: u32 = 0;
        // First call to get the required size
        GetTokenInformation(token, TokenGroups, ptr::null_mut(), 0, &mut needed);

        if GetLastError() != ERROR_INSUFFICIENT_BUFFER {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token,
            TokenGroups,
            buf.as_mut_ptr() as _,
            needed,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        Ok(buf)
    }
}

/// Sets the primary group on a token.
fn set_token_primary_group(token: HANDLE, sid: &[u8]) -> io::Result<()> {
    unsafe {
        let mut token_group = TOKEN_PRIMARY_GROUP {
            PrimaryGroup: sid.as_ptr() as _,
        };

        if SetTokenInformation(
            token,
            TokenPrimaryGroup,
            &mut token_group as *mut _ as *const _,
            std::mem::size_of::<TOKEN_PRIMARY_GROUP>() as u32,
        ) == 0
        {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        Ok(())
    }
}

/// Spawns a process with the given token and command line, waits for it, and returns its exit code.
fn spawn_process_as_user(token: HANDLE, cmd: &str) -> io::Result<u32> {
    let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut startup_info: STARTUPINFOW = std::mem::zeroed();
        startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process_info: PROCESS_INFORMATION = std::mem::zeroed();

        if CreateProcessAsUserW(
            token,
            ptr::null(),
            cmd_w.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            0,
            ptr::null(),
            ptr::null(),
            &startup_info,
            &mut process_info,
        ) == 0
        {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        CloseHandle(process_info.hThread);
        WaitForSingleObject(process_info.hProcess, INFINITE);

        let mut exit_code: u32 = 1;
        GetExitCodeProcess(process_info.hProcess, &mut exit_code);
        CloseHandle(process_info.hProcess);

        Ok(exit_code)
    }
}
