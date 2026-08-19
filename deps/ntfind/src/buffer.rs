// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use core::ops::Deref;
use core::ptr;
use core::slice;

use alloc::vec::Vec;
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, GetLastError, HANDLE};
use windows_sys::Win32::Globalization::{
    IS_TEXT_UNICODE_NOT_UNICODE_MASK, IS_TEXT_UNICODE_REVERSE_MASK, IS_TEXT_UNICODE_SIGNATURE,
    IS_TEXT_UNICODE_STATISTICS, IS_TEXT_UNICODE_UNICODE_MASK, IsDBCSLeadByte, IsTextUnicode,
    LCMAP_UPPERCASE, LCMapStringEx, MultiByteToWideChar,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType, ReadFile};

use crate::io::StatusResult;
use crate::io::{input_code_page, write_stderr_str};

/// A helper for wchar_t string buffers. It's rather crude in this state, but it gets the job done.
/// The original find would rely heavily on ulib's `PATH` type instead.
#[derive(Clone)]
pub struct WideString {
    vec: Vec<u16>,
}

impl WideString {
    pub fn new() -> Self {
        Self { vec: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            vec: Vec::with_capacity(capacity),
        }
    }

    pub fn from_slice(s: &[u16]) -> WideString {
        Self { vec: s.to_vec() }
    }

    pub fn wrap(vec: Vec<u16>) -> Self {
        Self { vec }
    }

    pub fn push_char(&mut self, ch: u16) {
        self.vec.push(ch);
    }

    pub fn push_str(&mut self, s: &str) {
        self.vec.extend(s.encode_utf16());
    }

    pub fn push_wide(&mut self, s: &[u16]) {
        self.vec.extend_from_slice(s);
    }

    pub fn as_slice(&self) -> &[u16] {
        &self.vec
    }

    pub fn clear(&mut self) {
        self.vec.clear();
    }

    pub unsafe fn set_len(&mut self, new_len: usize) {
        unsafe { self.vec.set_len(new_len) }
    }

    pub fn as_mut_ptr(&mut self) -> *mut u16 {
        self.vec.as_mut_ptr()
    }

    pub fn as_cstr(&mut self) -> WideStringNullTerminated<'_> {
        // Ensure null-termination. Unsafe, but... not? UB. Well if it is, it shouldn't.
        // The borrow ensures no one mutates the buffer while the null-terminated pointer is alive.
        let ptr = unsafe {
            self.push_char(0);
            self.vec.set_len(self.vec.len() - 1);
            &*self.vec.as_ptr()
        };
        WideStringNullTerminated { ptr }
    }

    pub fn upper(&self) -> WideString {
        unsafe {
            if self.vec.is_empty() {
                return Self::new();
            }

            let needed = LCMapStringEx(
                ptr::null(), // LOCALE_NAME_USER_DEFAULT,
                LCMAP_UPPERCASE,
                self.vec.as_ptr(),
                self.vec.len() as i32,
                ptr::null_mut(),
                0,
                ptr::null(),
                ptr::null(),
                0,
            );
            if needed <= 0 {
                return self.clone();
            }

            let mut dest = Vec::with_capacity(needed as usize);
            let written = LCMapStringEx(
                ptr::null(), // LOCALE_NAME_USER_DEFAULT,
                LCMAP_UPPERCASE,
                self.vec.as_ptr(),
                self.vec.len() as i32,
                dest.as_mut_ptr(),
                needed,
                ptr::null(),
                ptr::null(),
                0,
            );
            if written <= 0 {
                return self.clone();
            }

            dest.set_len(written as usize);
            Self::wrap(dest)
        }
    }
}

impl Deref for WideString {
    type Target = [u16];

    fn deref(&self) -> &Self::Target {
        &self.vec
    }
}

impl fmt::Write for WideString {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        self.vec.extend_from_slice(c.encode_utf16(&mut [0; 2]));
        Ok(())
    }
}

pub struct WideStringNullTerminated<'a> {
    ptr: &'a u16,
}

impl WideStringNullTerminated<'_> {
    pub fn get(&self) -> *const u16 {
        self.ptr
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamType {
    Unknown = -1,
    Ansi = 0,
    Unicode = 1,
}

/// `BufferStream` very loosely replicates ulib's `BUFFER_STREAM`.
/// This version however is quite a bit simpler and faster.
pub struct BufferStream {
    handle: HANDLE,
    buffer: Vec<u8>,

    /// Specifically for `read_line_ansi` for unicode conversion.
    unicode: Vec<u16>,

    /// Start of the valid data and current (incomplete) line.
    beg: usize,
    /// Offset of where the next `memchr` should continue scanning from.
    scan: usize,

    eof: bool,
    stream_type: StreamType, // -1 = not yet detected, 0 = ANSI, 1 = UTF-16LE
}

impl BufferStream {
    pub fn new(handle: HANDLE) -> Self {
        Self {
            handle,

            buffer: Vec::with_capacity(64 * 1024),
            unicode: Vec::new(),

            beg: 0,
            scan: 0,

            eof: false,
            stream_type: StreamType::Unknown,
        }
    }

    pub fn read_line(&mut self) -> StatusResult<Option<&[u16]>> {
        if self.stream_type == StreamType::Unknown {
            self.detect_stream_type()?;
        }

        let line = if self.stream_type == StreamType::Unicode {
            self.read_line_unicode()
        } else {
            self.read_line_ansi()
        };

        line.map(|opt| opt.map(|line| line.strip_suffix(&[0x0D]).unwrap_or(line)))
    }

    /// Detect whether the stream is ANSI or UTF-16LE.
    /// This replicates `BUFFER_STREAM::DetermineStreamType` and `BUFFER_STREAM::GetBuffer`.
    fn detect_stream_type(&mut self) -> StatusResult<()> {
        unsafe {
            self.stream_type = StreamType::Ansi;

            self.fill_buffer()?;
            if self.buffer.is_empty() {
                return Ok(());
            }

            // QUIRK / BUG:
            // find.exe would transparently use memory mapping for `FILE_TYPE_DISK` (see `FILE_STREAM::Initialize`).
            // This doesn't work well with Rust, as we cannot handle the `EXCEPTION_IN_PAGE_ERROR` SEH exception.
            // That's not a problem. ulib is quite poorly written and we can outperform it with `ReadFile` anyway.
            // `(Rtl)IsTextUnicode` also only inspects the first 256 bytes, so `ReadFile` doesn't loose us anything.
            //
            // Why does this matter? Because ulib contains two distinct and separat copies of the `RtlIsTextUnicode`
            // logic and their flags subtly differ. There's no explanation in the code why that is.
            //
            // NOTE that technically ulib also checks the file size and restricts mmap to <3GiB.
            // At find.exe's performance that will hardly matter.

            let mut result = u32::MAX;
            let is_unicode = IsTextUnicode(
                self.buffer.as_ptr() as *const _,
                self.buffer.len() as i32,
                &mut result,
            );

            if (result & IS_TEXT_UNICODE_SIGNATURE) != 0 {
                self.stream_type = StreamType::Unicode;
                if self.buffer.len() >= 2 {
                    self.beg = 2; // skip BOM
                }
                return Ok(());
            }

            if is_unicode == 0 {
                return Ok(());
            }

            let flags_match = if GetFileType(self.handle) == FILE_TYPE_DISK {
                ((result & IS_TEXT_UNICODE_UNICODE_MASK) != 0
                    || (result & IS_TEXT_UNICODE_REVERSE_MASK) != 0)
                    && (result & IS_TEXT_UNICODE_NOT_UNICODE_MASK) == 0
            } else {
                (result & IS_TEXT_UNICODE_STATISTICS) != 0
                    && (result & !IS_TEXT_UNICODE_STATISTICS) == 0
            };

            if flags_match && !self.buffer.iter().any(|&b| IsDBCSLeadByte(b) != 0) {
                self.stream_type = StreamType::Unicode;
            }

            Ok(())
        }
    }

    fn read_line_ansi(&mut self) -> StatusResult<Option<&[u16]>> {
        loop {
            if self.eof {
                return Ok(if self.beg >= self.buffer.len() {
                    None
                } else {
                    let beg = self.beg;
                    let end = self.buffer.len();
                    self.beg = self.buffer.len();
                    Some(self.convert_to_wide(beg, end))
                });
            }

            // Look for a line terminator and if found, yield that line.
            if let Some(off) = self.buffer[self.scan..]
                .iter()
                .position(|&c| c == 0x00 || c == 0x0A)
            {
                let beg = self.beg;
                let end = self.scan + off;
                self.beg = end + 1;
                self.scan = self.beg;
                return Ok(Some(self.convert_to_wide(beg, end)));
            }

            // `buffer[beg..]` has no terminator. Remember that for the next scan.
            self.scan = self.buffer.len();
            self.fill_buffer()?;
        }
    }

    fn read_line_unicode(&mut self) -> StatusResult<Option<&[u16]>> {
        // QUIRK / BUG:
        // ulib's `BUFFER_STREAM::ReadString` does this, right after finding a newline with `wcscspn`:
        //   if((BytesInBuffer & 0xfffe) != BytesInBuffer){
        //      BytesInBuffer++;
        //   }
        // Then later on:
        //   EndOfString = (BytesConsumed < BytesInBuffer);
        //
        // `BytesConsumed` in this case is in sizeof(wchar_t) units, so the former code can lead the latter
        // to believe that the end of the string was detected, leave a trailing byte in the buffer,
        // and corrupt the string and find.exe's output.
        //
        // The funny thing is that `BUFFER_STREAM::ReadWString` (= read into an array) got it "correct":
        //    BytesConsumed = BytesInBuffer&0xFFFE;
        //
        // Don't ask me about why it uses 0xfffe instead of ~1.
        // I suppose this means find.exe cannot handle lines >64K chars.

        loop {
            let beg = self.beg / 2;
            let scan = self.scan / 2;
            let end = self.buffer.len() / 2;
            let wide = unsafe {
                let ptr = self.buffer.as_ptr() as *const u16;
                assert!(ptr.is_aligned());
                slice::from_raw_parts(ptr, end)
            };

            if self.eof {
                return Ok(if beg >= wide.len() {
                    None
                } else {
                    self.beg = self.buffer.len();
                    Some(&wide[beg..])
                });
            }

            // Look for a line terminator and if found, yield that line.
            if let Some(off) = wide[scan..].iter().position(|&c| c == 0x00 || c == 0x0A) {
                let end = scan + off;
                self.beg = (end + 1) * 2;
                self.scan = self.beg;
                return Ok(Some(&wide[beg..end]));
            }

            // `buffer[beg..]` has no terminator. Remember that for the next scan.
            self.scan = self.buffer.len();
            self.fill_buffer()?;
        }
    }

    fn fill_buffer(&mut self) -> StatusResult<()> {
        unsafe {
            // Move the partial line to the beginning of the buffer.
            // The idea is that read() calls will either be very slow, and this doesn't matter,
            // or they'll be very fast with big chunks and we want to maximize the amount of space per syscall.
            if self.beg > 0 {
                self.buffer.copy_within(self.beg.., 0);
                self.buffer.set_len(self.buffer.len() - self.beg);
                self.scan -= self.beg;
                self.beg = 0;
            }

            self.buffer.reserve(32 * 1024);

            let spare = self.buffer.spare_capacity_mut();
            let mut bytes_read: u32 = 0;

            let ok = ReadFile(
                self.handle,
                spare.as_mut_ptr() as *mut _,
                spare.len() as u32,
                &mut bytes_read,
                ptr::null_mut(),
            );
            if ok == 0 {
                let err = GetLastError();
                if err == ERROR_BROKEN_PIPE {
                    self.eof = true;
                    return Ok(());
                }
                write_stderr_str("Unable to read file\r\n");
                return Err(2);
            }

            self.buffer.set_len(self.buffer.len() + bytes_read as usize);
            self.eof = bytes_read == 0;
            Ok(())
        }
    }

    fn convert_to_wide(&mut self, beg: usize, end: usize) -> &[u16] {
        let Some(bytes) = self.buffer.get(beg..end) else {
            return &[];
        };
        if bytes.is_empty() {
            return &[];
        }

        unsafe {
            let codepage = input_code_page();

            let wide_len = MultiByteToWideChar(
                codepage,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                ptr::null_mut(),
                0,
            );
            if wide_len <= 0 {
                return &[];
            }

            self.unicode.reserve(wide_len as usize);

            let wide_len = MultiByteToWideChar(
                codepage,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                self.unicode.as_mut_ptr(),
                wide_len,
            );
            if wide_len <= 0 {
                return &[];
            }

            self.unicode.set_len(wide_len as usize);
        }

        &self.unicode
    }
}
