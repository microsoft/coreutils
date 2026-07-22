// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::ptr;

use alloc::vec::Vec;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Globalization::WideCharToMultiByte;
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::Console::{
    GetConsoleCP, GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE,
    WriteConsoleW,
};

use crate::buffer::WideString;

pub type StatusResult<T> = core::result::Result<T, i32>;

pub trait IntoStatus {
    fn into_status(self) -> i32;
}

impl<T> IntoStatus for core::result::Result<T, i32> {
    fn into_status(self) -> i32 {
        match self {
            Ok(_) => 0,
            Err(code) => code,
        }
    }
}

pub struct OutputHandle {
    handle: HANDLE,
    is_console: bool,
}

impl OutputHandle {
    const fn empty() -> Self {
        Self {
            handle: ptr::null_mut(),
            is_console: false,
        }
    }

    fn new(handle: STD_HANDLE) -> Self {
        let handle = unsafe { GetStdHandle(handle) };
        let mut mode: u32 = 0;
        let is_console = unsafe { GetConsoleMode(handle, &mut mode) != 0 };
        Self { handle, is_console }
    }
}

struct IOState {
    stdout: OutputHandle,
    stderr: OutputHandle,
    input_code_page: u32,
}

static mut IO: IOState = IOState {
    stdout: OutputHandle::empty(),
    stderr: OutputHandle::empty(),
    input_code_page: 0,
};

/// ulib initializes IO handles lazily using global statics inside IO functions.
/// We do it eagerly and explicitly on startup.
pub fn io_init() {
    unsafe {
        IO.stdout = OutputHandle::new(STD_OUTPUT_HANDLE);
        IO.stderr = OutputHandle::new(STD_ERROR_HANDLE);

        // QUIRK / BUG: ulib does not check `GetConsoleCP()` for errors.
        // That's technically fine. The error code is 0 which equates to CP_ACP.
        // This only works because find.exe just so happens to only use WideCharToMultiByte.
        IO.input_code_page = GetConsoleCP();
    }
}

pub fn input_code_page() -> u32 {
    unsafe { IO.input_code_page }
}

#[allow(static_mut_refs)]
pub fn stdout_handle() -> &'static OutputHandle {
    unsafe { &IO.stdout }
}

#[allow(static_mut_refs)]
pub fn stderr_handle() -> &'static OutputHandle {
    unsafe { &IO.stderr }
}

/// Loosely matches `STREAM::WriteString` (the parts we need at least).
pub fn write_to_handle(h: &OutputHandle, wide: &[u16]) {
    if wide.is_empty() {
        return;
    }

    unsafe {
        let mut written: u32 = 0;

        // QUIRK / BUG: ulib calls WriteConsoleW in chunks of 40 chars (WHY WOULD YOU DO THAT; this hurts the soul).
        // See `STREAM_MESSAGE::DisplayString`. It pays no attention to surrogate pairs, which is why find.exe may
        // result in broken Unicode output. It's also why the /? help message results in "...specified string.\r\r\n".
        // The 40-char boundaries end up splitting the "\r\n" and ConPTY turns the sole "\n" into "\r\n".
        if h.is_console {
            WriteConsoleW(
                h.handle,
                wide.as_ptr(),
                wide.len() as u32,
                &mut written,
                ptr::null(),
            );
            return;
        }

        // `STREAM::WriteString` calls `WSTRING::QuerySTR` which calls `ConvertUnicodeToOemN`
        // which calls `ConvertToOemWithConsoleCP`, because find.cxx called `SetConsoleConversions`.
        // QUIRK / BUG: Yep, ulib uses the stdin CP for stdout encoding.
        let len = WideCharToMultiByte(
            IO.input_code_page,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
            ptr::null_mut(),
        );
        if len <= 0 {
            return;
        }

        let mut buf = Vec::with_capacity(len as usize);
        let len = WideCharToMultiByte(
            IO.input_code_page,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            buf.as_mut_ptr(),
            len,
            ptr::null(),
            ptr::null_mut(),
        );
        if len <= 0 {
            return;
        }

        buf.set_len(len as usize);
        WriteFile(
            h.handle,
            buf.as_ptr(),
            len as u32,
            &mut written,
            ptr::null_mut(),
        );
    }
}

pub fn write_str(h: &OutputHandle, s: &str) {
    let mut wide = WideString::new();
    wide.push_str(s);
    write_to_handle(h, &wide);
}

pub fn write_stdout_str(s: &str) {
    write_str(stdout_handle(), s);
}

pub fn write_stderr_str(s: &str) {
    write_str(stderr_handle(), s);
}
