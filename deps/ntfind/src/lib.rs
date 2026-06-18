// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]

extern crate alloc;

mod argv;
mod buffer;
mod io;
mod path;

use core::fmt::Write as _;
use core::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Globalization::{
    CSTR_EQUAL, CompareStringW, LOCALE_USER_DEFAULT, NORM_IGNORECASE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE, FILE_FLAG_OPEN_NO_RECALL,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};

use crate::argv::parse_command_line;
use crate::buffer::{BufferStream, WideString};
use crate::io::{
    IntoStatus as _, StatusResult, io_init, stderr_handle, stdout_handle, write_stderr_str,
    write_to_handle,
};
use crate::path::is_drive_path;

const MSG_USAGE: &str = concat!(
    "Searches for a text string in a file or files.\r\n",
    "\r\n",
    "FIND [/V] [/C] [/N] [/I] [/OFF[LINE]] \"string\" [[drive:][path]filename[ ...]]\r\n",
    "\r\n",
    "  /V         Displays all lines NOT containing the specified string.\r\n",
    "  /C         Displays only the count of lines containing the string.\r\n",
    "  /N         Displays line numbers with the displayed lines.\r\n",
    "  /I         Ignores the case of characters when searching for the string.\r\n",
    "  /OFF[LINE] Do not skip files with offline attribute set.\r\n",
    "  \"string\"   Specifies the text string to find.\r\n",
    "  [drive:][path]filename\r\n",
    "             Specifies a file or files to search.\r\n",
    "\r\n",
    "If a path is not specified, FIND searches the text typed at the prompt\r\n",
    "or piped from another command.\r\n"
);

pub fn ntfind_main() -> i32 {
    main().into_status()
}

fn main() -> StatusResult<()> {
    io_init();

    let (mut state, mut files) = parse_command_line()?;

    if files.is_empty() {
        let h_stdin = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let lines_found = search_stream(&mut state, h_stdin)?;

        if !state.output_lines {
            let mut out = WideString::new();
            _ = write!(out, "{}\r\n", lines_found);
            write_to_handle(stdout_handle(), &out);
        }
    } else {
        let mut print_skip_warning = false;
        let mut out = WideString::new();

        for file in &mut files {
            let display_path = file.upper();
            let cstr = file.as_cstr();
            let attrs = unsafe { GetFileAttributesW(cstr.get()) };

            // Check if this is a directory
            if attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0 {
                out.clear();
                if is_drive_path(file) {
                    out.push_str("File not found - ");
                } else {
                    out.push_str("Access denied - ");
                }
                out.push_wide(&display_path);
                out.push_str("\r\n");
                write_to_handle(stderr_handle(), &out);
                continue;
            }

            // Handle offline files
            let mut offline_skipped = false;
            let mut h_file = INVALID_HANDLE_VALUE;

            if state.skip_offline_files
                && attrs != INVALID_FILE_ATTRIBUTES
                && (attrs & FILE_ATTRIBUTE_OFFLINE) != 0
            {
                offline_skipped = true;
            } else {
                h_file = unsafe {
                    CreateFileW(
                        cstr.get(),
                        GENERIC_READ,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        ptr::null(),
                        OPEN_EXISTING,
                        FILE_FLAG_OPEN_NO_RECALL,
                        ptr::null_mut(),
                    )
                };
            }

            if h_file == INVALID_HANDLE_VALUE {
                if is_dos5_compatible_filename(file) {
                    if offline_skipped {
                        print_skip_warning = true;
                    } else {
                        out.clear();
                        out.push_str("File not found - ");
                        out.push_wide(&display_path);
                        out.push_str("\r\n");
                        write_to_handle(stderr_handle(), &out);
                    }
                } else {
                    write_stderr_str("FIND: Parameter format not correct\r\n");
                    break;
                }
                continue;
            }

            if state.output_lines {
                out.clear();
                out.push_str("\r\n---------- ");
                out.push_wide(&display_path);
                out.push_str("\r\n");
                write_to_handle(stdout_handle(), &out);
            }

            let lines_found = search_stream(&mut state, h_file)?;

            if !state.output_lines {
                out.clear();
                out.push_str("\r\n---------- ");
                out.push_wide(&display_path);
                out.push_str(": ");
                _ = write!(out, "{}\r\n", lines_found);
                write_to_handle(stdout_handle(), &out);
            }

            unsafe { CloseHandle(h_file) };
        }

        if print_skip_warning {
            write_stderr_str(
                "Files with offline attribute were skipped.\r\nUse /OFFLINE for not skipping such files.\r\n",
            );
        }
    }

    if state.found_any { Ok(()) } else { Err(1) }
}

fn search_stream(state: &mut argv::FindState, handle: HANDLE) -> StatusResult<u32> {
    let mut line_count: u32 = 0;
    let mut found_count: u32 = 0;
    let pattern_len = state.pattern.len();
    let mut reader = BufferStream::new(handle);
    let mut out = WideString::new();
    let compare_flags = if state.case_sensitive {
        0
    } else {
        NORM_IGNORECASE
    };

    while let Some(line) = reader.read_line()? {
        line_count += 1;

        // Look for pattern in the current line. A 0-length pattern ("") never matches.
        // Scan from end to start (preserving original behavior).
        let mut found = false;

        if pattern_len > 0 && line.len() >= pattern_len {
            let last_pos = line.len() - pattern_len;
            let mut pos = last_pos as isize;

            while pos >= 0 && !found {
                let result = unsafe {
                    CompareStringW(
                        LOCALE_USER_DEFAULT,
                        compare_flags,
                        state.pattern.as_ptr(),
                        pattern_len as i32,
                        line[pos as usize..].as_ptr(),
                        pattern_len as i32,
                    )
                };
                if result == CSTR_EQUAL {
                    found = true;
                }
                pos -= 1;
            }
        }

        // Output if (positive search && found) || (negative search && !found)
        if (state.lines_containing_pattern && found) || (!state.lines_containing_pattern && !found)
        {
            found_count += 1;
            if state.output_lines {
                out.clear();
                if state.output_line_numbers {
                    _ = write!(out, "[{}]", line_count);
                }
                out.push_wide(line);
                out.push_str("\r\n");
                write_to_handle(stdout_handle(), &out);
            }
        }
    }

    if found_count > 0 {
        state.found_any = true;
    }

    Ok(found_count)
}

/// Matches `IsDos5CompatibleFileName` in file.cxx.
fn is_dos5_compatible_filename(path: &[u16]) -> bool {
    path.first() != Some(&(b'"' as u16))
}
