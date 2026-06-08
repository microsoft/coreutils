// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_main]

use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    unsafe {
        let status = ntfind_main();
        TerminateProcess(GetCurrentProcess(), status as u32);
        core::hint::unreachable_unchecked();
    }
}
