// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;
use std::ffi::{OsString, c_void};
use std::io;
use std::mem;
use std::ptr;
use std::slice;

use clap::Command;
use uucore::Args;
use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_TYPE_DISK, GetFileType, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004_u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC0000023_u32 as i32;
const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
const OBJECT_TYPE_INFORMATION: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct SystemHandleTableEntryInfoEx {
    object: *mut c_void,
    unique_process_id: usize,
    handle_value: usize,
    granted_access: u32,
    creator_back_trace_index: u16,
    object_type_index: u16,
    handle_attributes: u32,
    reserved: u32,
}

#[repr(C)]
struct SystemHandleInformationEx {
    number_of_handles: usize,
    reserved: usize,
    handles: [SystemHandleTableEntryInfoEx; 1],
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectTypeInformation {
    type_name: UnicodeString,
}

unsafe extern "system" {
    fn NtQuerySystemInformation(
        system_information_class: u32,
        system_information: *mut c_void,
        system_information_length: u32,
        return_length: *mut u32,
    ) -> i32;

    fn NtQueryObject(
        handle: HANDLE,
        object_information_class: u32,
        object_information: *mut c_void,
        object_information_length: u32,
        return_length: *mut u32,
    ) -> i32;
}

#[derive(Debug, Eq, PartialEq)]
enum LsofAction {
    Help,
    Version,
    List(LsofOptions),
}

#[derive(Debug, Default, Eq, PartialEq)]
struct LsofOptions {
    pid: Option<u32>,
    command: Option<String>,
    paths: Vec<String>,
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

struct DuplicatedHandle(HANDLE);

impl Drop for DuplicatedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

pub fn uumain<T: Args>(args: T) -> i32 {
    let mut args = args.into_iter();
    let program = args.next().unwrap_or_else(|| OsString::from("lsof"));
    let program = program.to_string_lossy();
    let args: Vec<OsString> = args.collect();

    match parse_args(&args) {
        Ok(LsofAction::Help) => {
            print_help(&program);
            0
        }
        Ok(LsofAction::Version) => {
            println!("lsof {VERSION}");
            0
        }
        Ok(LsofAction::List(options)) => list_open_files(&options),
        Err(err) => {
            eprintln!("lsof: {err}");
            1
        }
    }
}

pub fn uu_app() -> Command {
    Command::new("lsof")
        .version(VERSION)
        .about("List open files held by Windows processes")
}

fn parse_args(args: &[OsString]) -> Result<LsofAction, String> {
    let mut options = LsofOptions::default();
    let mut options_done = false;
    let mut idx = 0;

    while idx < args.len() {
        let arg = args[idx].to_string_lossy();

        if !options_done {
            match arg.as_ref() {
                "--" => {
                    options_done = true;
                    idx += 1;
                    continue;
                }
                "--help" | "-h" => return Ok(LsofAction::Help),
                "--version" | "-V" => return Ok(LsofAction::Version),
                "-p" => {
                    idx += 1;
                    let Some(value) = args.get(idx) else {
                        return Err(format!("option '{arg}' requires an argument"));
                    };
                    options.pid = Some(parse_pid(&value.to_string_lossy())?);
                    idx += 1;
                    continue;
                }
                "-c" => {
                    idx += 1;
                    let Some(value) = args.get(idx) else {
                        return Err(format!("option '{arg}' requires an argument"));
                    };
                    options.command = Some(value.to_string_lossy().into_owned());
                    idx += 1;
                    continue;
                }
                _ if arg.starts_with("-p") && arg.len() > 2 => {
                    options.pid = Some(parse_pid(&arg[2..])?);
                    idx += 1;
                    continue;
                }
                _ if arg.starts_with("-c") && arg.len() > 2 => {
                    options.command = Some(arg[2..].to_string());
                    idx += 1;
                    continue;
                }
                _ if arg.starts_with('-') => {
                    return Err(format!("unsupported option '{arg}'"));
                }
                _ => {}
            }
        }

        options.paths.push(arg.into_owned());
        idx += 1;
    }

    Ok(LsofAction::List(options))
}

fn parse_pid(value: &str) -> Result<u32, String> {
    let pid: u32 = value
        .parse()
        .map_err(|err| format!("invalid process id '{value}': {err}"))?;
    if pid == 0 {
        return Err("process id must be greater than zero".to_string());
    }
    Ok(pid)
}

fn list_open_files(options: &LsofOptions) -> i32 {
    let handles = match query_system_handles() {
        Ok(handles) => handles,
        Err(err) => {
            eprintln!("lsof: failed to query system handles: {err}");
            return 1;
        }
    };

    println!("{:<24} {:>8} {:<4} NAME", "COMMAND", "PID", "TYPE");

    let mut process_cache = HashMap::new();
    let mut process_handles = HashMap::new();
    let mut status = 0;

    for handle in handles {
        let pid = handle.unique_process_id as u32;
        if options.pid.is_some_and(|wanted| wanted != pid) {
            continue;
        }

        let process = match get_process_handle(pid, &mut process_handles) {
            Some(process) => process,
            None => continue,
        };

        let command = process_cache
            .entry(pid)
            .or_insert_with(|| process_name(process.0, pid));
        if let Some(filter) = &options.command
            && !command
                .to_ascii_lowercase()
                .contains(&filter.to_ascii_lowercase())
        {
            continue;
        }

        let Some(dup) = duplicate_process_handle(process.0, handle.handle_value) else {
            continue;
        };

        if !is_file_handle(dup.0) || unsafe { GetFileType(dup.0) } != FILE_TYPE_DISK {
            continue;
        }

        let Some(path) = final_path_name(dup.0) else {
            continue;
        };

        if !matches_paths(&path, &options.paths) {
            continue;
        }

        if println_open_file(command, pid, &path).is_err() {
            status = 1;
            break;
        }
    }

    status
}

fn get_process_handle(
    pid: u32,
    cache: &mut HashMap<u32, Option<ProcessHandle>>,
) -> Option<&ProcessHandle> {
    cache
        .entry(pid)
        .or_insert_with(|| {
            let handle = unsafe {
                OpenProcess(
                    PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    pid,
                )
            };
            (!handle.is_null()).then_some(ProcessHandle(handle))
        })
        .as_ref()
}

fn query_system_handles() -> io::Result<Vec<SystemHandleTableEntryInfoEx>> {
    let mut size = 1024 * 1024;

    loop {
        let mut buffer = vec![0u8; size];
        let mut return_len = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut return_len,
            )
        };

        if status == 0 {
            let info = unsafe { &*(buffer.as_ptr().cast::<SystemHandleInformationEx>()) };
            let count = info.number_of_handles;
            let first = ptr::addr_of!(info.handles).cast::<SystemHandleTableEntryInfoEx>();
            let handles = unsafe { slice::from_raw_parts(first, count) }.to_vec();
            return Ok(handles);
        }

        if status == STATUS_INFO_LENGTH_MISMATCH || status == STATUS_BUFFER_TOO_SMALL {
            size = (return_len as usize).max(size * 2);
            continue;
        }

        return Err(io::Error::from_raw_os_error(status));
    }
}

fn duplicate_process_handle(process: HANDLE, handle_value: usize) -> Option<DuplicatedHandle> {
    let mut duplicated: HANDLE = ptr::null_mut();
    let ok = unsafe {
        DuplicateHandle(
            process,
            handle_value as HANDLE,
            GetCurrentProcess(),
            &mut duplicated,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    (ok != 0 && !duplicated.is_null()).then_some(DuplicatedHandle(duplicated))
}

fn is_file_handle(handle: HANDLE) -> bool {
    let mut buffer = vec![0u8; 1024];
    let mut return_len = 0u32;
    let status = unsafe {
        NtQueryObject(
            handle,
            OBJECT_TYPE_INFORMATION,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut return_len,
        )
    };

    if status == STATUS_INFO_LENGTH_MISMATCH || status == STATUS_BUFFER_TOO_SMALL {
        buffer.resize(return_len as usize, 0);
        let status = unsafe {
            NtQueryObject(
                handle,
                OBJECT_TYPE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut return_len,
            )
        };
        if status != 0 {
            return false;
        }
    } else if status != 0 {
        return false;
    }

    let info = unsafe { &*(buffer.as_ptr().cast::<ObjectTypeInformation>()) };
    let len = usize::from(info.type_name.length) / mem::size_of::<u16>();
    if info.type_name.buffer.is_null() {
        return false;
    }

    let name = unsafe { slice::from_raw_parts(info.type_name.buffer, len) };
    String::from_utf16_lossy(name) == "File"
}

fn final_path_name(handle: HANDLE) -> Option<String> {
    let mut size = 512usize;

    loop {
        let mut buffer = vec![0u16; size];
        let len = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                VOLUME_NAME_DOS,
            )
        };

        if len == 0 {
            return None;
        }

        let len = len as usize;
        if len < buffer.len() {
            buffer.truncate(len);
            return Some(clean_final_path(&String::from_utf16_lossy(&buffer)));
        }

        size = len + 1;
    }
}

fn clean_final_path(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

fn process_name(process: HANDLE, pid: u32) -> String {
    let mut size = 260u32;

    loop {
        let mut buffer = vec![0u16; size as usize];
        let mut len = size;
        let ok = unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len) };

        if ok != 0 {
            buffer.truncate(len as usize);
            let full = String::from_utf16_lossy(&buffer);
            return full
                .rsplit(['\\', '/'])
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(&full)
                .to_string();
        }

        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(122) {
            size *= 2;
            continue;
        }

        return format!("{pid}");
    }
}

fn matches_paths(path: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }

    let path = path.to_ascii_lowercase();
    filters
        .iter()
        .any(|filter| path.contains(&filter.to_ascii_lowercase()))
}

fn println_open_file(command: &str, pid: u32, path: &str) -> io::Result<()> {
    use std::io::Write as _;

    let mut out = io::stdout().lock();
    writeln!(out, "{command:<24} {pid:>8} {:<4} {path}", "REG")
}

fn print_help(program: &str) {
    println!(
        "\
Usage: {program} [OPTION]... [FILE]...

List open files held by Windows processes.

With no FILE operands, list accessible open disk files. FILE operands are
matched as case-insensitive substrings of the resolved Windows path.

Options:
  -p PID          list files opened by PID
  -c COMMAND      list files opened by commands matching COMMAND
      --help      display this help and exit
      --version   output version information and exit"
    );
}

#[cfg(test)]
mod tests {
    use super::{LsofAction, LsofOptions, clean_final_path, matches_paths, parse_args, parse_pid};
    use std::ffi::OsString;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_empty_list() {
        assert_eq!(
            parse_args(&args(&[])),
            Ok(LsofAction::List(LsofOptions::default()))
        );
    }

    #[test]
    fn parses_filters() {
        assert_eq!(
            parse_args(&args(&["-p", "123", "-c", "cmd", "Cargo.toml"])),
            Ok(LsofAction::List(LsofOptions {
                pid: Some(123),
                command: Some("cmd".to_string()),
                paths: vec!["Cargo.toml".to_string()],
            }))
        );
        assert_eq!(
            parse_args(&args(&["-p123", "-ccmd"])),
            Ok(LsofAction::List(LsofOptions {
                pid: Some(123),
                command: Some("cmd".to_string()),
                paths: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_end_of_options() {
        assert_eq!(
            parse_args(&args(&["--", "-literal"])),
            Ok(LsofAction::List(LsofOptions {
                pid: None,
                command: None,
                paths: vec!["-literal".to_string()],
            }))
        );
    }

    #[test]
    fn rejects_invalid_pid() {
        assert!(parse_pid("0").is_err());
        assert!(parse_pid("abc").is_err());
    }

    #[test]
    fn matches_path_filters_case_insensitively() {
        assert!(matches_paths(r"C:\Temp\File.txt", &[String::from("temp")]));
        assert!(!matches_paths(
            r"C:\Temp\File.txt",
            &[String::from("other")]
        ));
    }

    #[test]
    fn cleans_extended_path_prefix() {
        assert_eq!(
            clean_final_path(r"\\?\C:\Temp\File.txt"),
            r"C:\Temp\File.txt"
        );
        assert_eq!(clean_final_path(r"C:\Temp\File.txt"), r"C:\Temp\File.txt");
    }
}
