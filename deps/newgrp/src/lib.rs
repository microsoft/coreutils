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
    DuplicateTokenEx, LookupAccountNameW, SecurityImpersonation, SetTokenInformation, TokenPrimary,
    TokenPrimaryGroup, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
    TOKEN_PRIMARY_GROUP, TOKEN_QUERY,
};
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
                .help("Command to execute")
                .action(ArgAction::Set),
        )
}

#[uucore::main(no_signals)]
pub fn uumain(args: impl uucore::Args) -> UResult<()> {
    let matches = uu_app().get_matches_from(args);
    let group = matches.get_one::<String>("group");
    let command = matches.get_one::<String>("command");

    let mut token: HANDLE = std::ptr::null_mut();
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
                    "OpenProcessToken failed: {}",
                    io::Error::from_raw_os_error(GetLastError() as i32)
                ),
            ));
        }
    }

    let mut new_token: HANDLE = std::ptr::null_mut();
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
            CloseHandle(token);
            return Err(USimpleError::new(
                1,
                format!(
                    "DuplicateTokenEx failed: {}",
                    io::Error::from_raw_os_error(GetLastError() as i32)
                ),
            ));
        }
        CloseHandle(token);
    }

    if let Some(g) = group {
        if let Err(e) = change_primary_group(new_token, g) {
            unsafe {
                CloseHandle(new_token);
            }
            return Err(USimpleError::new(
                1,
                format!("failed to change primary group: {}", e),
            ));
        }
    }

    // Spawn the shell or command
    let cmd = if let Some(c) = command {
        format!("cmd.exe /c {}", c)
    } else {
        "cmd.exe".to_string()
    };

    let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut startup_info: STARTUPINFOW = std::mem::zeroed();
        startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process_info: PROCESS_INFORMATION = std::mem::zeroed();

        if CreateProcessAsUserW(
            new_token,
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
            let err = GetLastError();
            CloseHandle(new_token);
            return Err(USimpleError::new(
                1,
                format!(
                    "CreateProcessAsUserW failed: {}",
                    io::Error::from_raw_os_error(err as i32)
                ),
            ));
        }

        CloseHandle(new_token);
        CloseHandle(process_info.hThread);

        WaitForSingleObject(process_info.hProcess, INFINITE);

        let mut exit_code = 0;
        GetExitCodeProcess(process_info.hProcess, &mut exit_code);
        CloseHandle(process_info.hProcess);

        if exit_code != 0 {
            std::process::exit(exit_code as i32);
        }
    }

    Ok(())
}

fn change_primary_group(token: HANDLE, group: &str) -> io::Result<()> {
    unsafe {
        let group_w: Vec<u16> = group.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sid_size = 0;
        let mut domain_size = 0;
        let mut pe_use = 0;

        // First call to get required sizes
        LookupAccountNameW(
            ptr::null(),
            group_w.as_ptr(),
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
            group_w.as_ptr(),
            sid.as_mut_ptr() as _,
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut pe_use,
        ) == 0 {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        let mut token_group = TOKEN_PRIMARY_GROUP {
            PrimaryGroup: sid.as_mut_ptr() as _,
        };

        if SetTokenInformation(
            token,
            TokenPrimaryGroup,
            &mut token_group as *mut _ as *const _,
            std::mem::size_of::<TOKEN_PRIMARY_GROUP>() as u32,
        ) == 0 {
            return Err(io::Error::from_raw_os_error(GetLastError() as i32));
        }

        Ok(())
    }
}
