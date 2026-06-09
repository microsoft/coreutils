// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use clap::{Arg, Command};
use std::process;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows_sys::Win32::Security::{
    LookupAccountNameW, SetTokenInformation, TokenPrimaryGroup,
    TOKEN_ADJUST_DEFAULT, TOKEN_PRIMARY_GROUP, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub fn uu_app() -> Command {
    Command::new("newgrp")
        .about("Log in to a new group")
        .arg(
            Arg::new("group")
                .help("The group to log into")
                .index(1)
                .required(false),
        )
}

pub fn uumain(args: impl uucore::Args) -> i32 {
    let matches = uu_app().get_matches_from(args);
    let group = matches.get_one::<String>("group");

    if let Some(g) = group {
        if let Err(e) = change_primary_group(g) {
            eprintln!("newgrp: failed to change primary group: {}", e);
            return 1;
        }
    }

    // Spawn the shell
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "cmd.exe".to_string());
    
    let mut child = match process::Command::new(shell).spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("newgrp: failed to execute shell: {}", e);
            return 1;
        }
    };

    match child.wait() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("newgrp: wait failed: {}", e);
            1
        }
    }
}

fn change_primary_group(group: &str) -> Result<(), String> {
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
            return Err(format!("LookupAccountNameW failed getting size: {}", GetLastError()));
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
            return Err(format!("LookupAccountNameW failed: {}", GetLastError()));
        }

        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_ADJUST_DEFAULT, &mut token) == 0 {
            return Err(format!("OpenProcessToken failed: {}", GetLastError()));
        }

        let mut token_group = TOKEN_PRIMARY_GROUP {
            PrimaryGroup: sid.as_mut_ptr() as _,
        };

        let res = SetTokenInformation(
            token,
            TokenPrimaryGroup,
            &mut token_group as *mut _ as *const _,
            std::mem::size_of::<TOKEN_PRIMARY_GROUP>() as u32,
        );

        CloseHandle(token);

        if res == 0 {
            return Err(format!("SetTokenInformation failed: {}", GetLastError()));
        }

        Ok(())
    }
}
