// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsString;
use std::ptr;
use std::slice;

use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::w;

unsafe extern "C" {
    pub fn wcslen(buf: *const u16) -> usize;
}

#[derive(Debug, PartialEq, Eq)]
enum Flavor {
    Ambiguous,
    Dos,
    Gnu,
}

#[derive(Clone, Copy)]
struct RawFindToken<'a> {
    raw: &'a [u16],
    starts_quoted: bool,
    has_backslash_quote_escape: bool,
}

pub fn is_ntfind_invocation() -> bool {
    match find_heuristic() {
        Flavor::Ambiguous => default_find_is_nt(),
        Flavor::Dos => true,
        Flavor::Gnu => false,
    }
}

fn default_find_is_nt() -> bool {
    // DefaultFind registry value: 0 (or missing) = DOS find, 1 = GNU find.
    let mut val: u32 = 0;
    let mut size = 4u32;
    let ret = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\coreutils"),
            w!("DefaultFind"),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            &mut val as *mut u32 as *mut _,
            &mut size,
        )
    };
    ret != 0 || val == 0
}

fn find_heuristic() -> Flavor {
    let cmd_line = unsafe {
        let p = GetCommandLineW();
        let len = wcslen(p) as usize;
        slice::from_raw_parts(p, len)
    };

    find_heuristic_impl(cmd_line)
}

fn find_heuristic_impl(cmd_line: &[u16]) -> Flavor {
    // Strip the program name
    let rest: &[u16] = next_raw_find_token(cmd_line).map_or(&[], |(_, rest)| rest);

    let Some((first, mut rest)) = next_raw_find_token(rest) else {
        return Flavor::Ambiguous;
    };
    if is_dos_find_switch(first.raw) {
        return Flavor::Dos;
    }
    if !first.starts_quoted {
        return Flavor::Gnu;
    }
    if first.has_backslash_quote_escape {
        return Flavor::Gnu;
    }

    let mut saw_rest_token = false;
    let mut saw_dash_token = false;
    while let Some((token, next)) = next_raw_find_token(rest) {
        saw_rest_token = true;
        if is_gnu_find_expression_token(token.raw) {
            return Flavor::Gnu;
        }
        if token.raw.first().is_some_and(|&c| c == b'-' as u16) {
            saw_dash_token = true;
        }
        rest = next;
    }

    if saw_rest_token && !saw_dash_token {
        Flavor::Dos
    } else {
        Flavor::Ambiguous
    }
}

fn next_raw_find_token(mut input: &[u16]) -> Option<(RawFindToken<'_>, &[u16])> {
    while input.first().is_some_and(|&c| is_whitespace(c)) {
        input = &input[1..];
    }
    if input.is_empty() {
        return None;
    }

    let starts_quoted = input[0] == b'"' as u16;
    let mut quoted = false;
    let mut has_backslash_quote_escape = false;
    let mut pos = 0;

    while pos < input.len() {
        let ch = input[pos];
        if !quoted && is_whitespace(ch) {
            break;
        }

        if quoted && ch == b'\\' as u16 && input.get(pos + 1) == Some(&(b'"' as u16)) {
            has_backslash_quote_escape = true;
            pos += 2;
            continue;
        }

        if ch == b'"' as u16 {
            if quoted && input.get(pos + 1) == Some(&(b'"' as u16)) {
                pos += 2;
                continue;
            }
            quoted = !quoted;
        }

        pos += 1;
    }

    Some((
        RawFindToken {
            raw: &input[..pos],
            starts_quoted,
            has_backslash_quote_escape,
        },
        &input[pos..],
    ))
}

const DOS_FIND_TOKENS: &[&[u8]] = &[b"/?", b"/C", b"/I", b"/N", b"/OFF", b"/OFFLINE", b"/V"];

fn is_dos_find_switch(token: &[u16]) -> bool {
    DOS_FIND_TOKENS
        .iter()
        .any(|&pattern| token_eq_insensitive(token, pattern))
}

const GNU_FIND_TOKENS: &[&[u8]] = &[
    b"!",
    b"(",
    b")",
    b",",
    b"--help",
    b"--version",
    b"-D",
    b"-H",
    b"-L",
    b"-P",
    b"-a",
    b"-amin",
    b"-and",
    b"-anewer",
    b"-atime",
    b"-cmin",
    b"-cnewer",
    b"-ctime",
    b"-daystart",
    b"-delete",
    b"-depth",
    b"-empty",
    b"-exec",
    b"-execdir",
    b"-executable",
    b"-false",
    b"-fls",
    b"-follow",
    b"-fprint",
    b"-fprint0",
    b"-fprintf",
    b"-fstype",
    b"-gid",
    b"-group",
    b"-ilname",
    b"-iname",
    b"-inum",
    b"-ipath",
    b"-iregex",
    b"-iwholename",
    b"-links",
    b"-lname",
    b"-ls",
    b"-maxdepth",
    b"-mindepth",
    b"-mmin",
    b"-mount",
    b"-mtime",
    b"-name",
    b"-newer",
    b"-nogroup",
    b"-noleaf",
    b"-not",
    b"-nouser",
    b"-o",
    b"-ok",
    b"-okdir",
    b"-or",
    b"-path",
    b"-perm",
    b"-print",
    b"-print0",
    b"-printf",
    b"-prune",
    b"-quit",
    b"-readable",
    b"-regex",
    b"-size",
    b"-true",
    b"-type",
    b"-uid",
    b"-used",
    b"-user",
    b"-version",
    b"-wholename",
    b"-writable",
    b"-xdev",
    b"-xtype",
];

fn is_gnu_find_expression_token(token: &[u16]) -> bool {
    GNU_FIND_TOKENS
        .iter()
        .any(|&pattern| token_eq(token, pattern))
}

pub fn is_ntsort_invocation(args: &[OsString]) -> bool {
    match sort_heuristic(args) {
        Flavor::Ambiguous => default_sort_is_nt(),
        Flavor::Dos => true,
        Flavor::Gnu => false,
    }
}

fn default_sort_is_nt() -> bool {
    // DefaultSort registry value: 0 (or missing) = DOS sort, 1 = GNU sort.
    let mut val: u32 = 0;
    let mut size = 4u32;
    let ret = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\coreutils"),
            w!("DefaultSort"),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            &mut val as *mut u32 as *mut _,
            &mut size,
        )
    };
    ret != 0 || val == 0
}

fn sort_heuristic(args: &[OsString]) -> Flavor {
    for arg in args.iter().skip(1) {
        let arg = arg.as_encoded_bytes();
        let Some((&first, rest)) = arg.split_first() else {
            continue;
        };
        if first == b'/' && !rest.is_empty() {
            return Flavor::Dos;
        }
        if first == b'-' && !rest.is_empty() {
            return Flavor::Gnu;
        }
        if first == b'+' && rest.first().is_some_and(u8::is_ascii_digit) {
            return Flavor::Gnu;
        }
    }
    Flavor::Ambiguous
}

fn is_whitespace(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16
}

fn token_eq(token: &[u16], pattern: &[u8]) -> bool {
    token.len() == pattern.len()
        && token
            .iter()
            .zip(pattern.iter())
            .all(|(&c, &p)| c == p as u16)
}

fn token_eq_insensitive(token: &[u16], pattern: &[u8]) -> bool {
    token.len() == pattern.len()
        && token.iter().zip(pattern.iter()).all(|(&c, &p)| {
            let c = if c >= b'a' as u16 && c <= b'z' as u16 {
                c - 0x20
            } else {
                c
            };
            c == p as u16
        })
}

#[cfg(test)]
mod tests {
    use super::{Flavor, find_heuristic_impl};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn find_heuristic() {
        for (cmd, flavor) in [
            (r#"find.exe / -name *.txt"#, Flavor::Gnu),
            (r#"find.exe /logs -type f"#, Flavor::Gnu),
            (r#"find.exe "/" file.txt"#, Flavor::Dos),
            (r#"find.exe /C needle file.txt"#, Flavor::Dos),
            (r#"find.exe /I needle file.txt"#, Flavor::Dos),
            (r#"find.exe /N needle file.txt"#, Flavor::Dos),
            (r#"find.exe /V needle file.txt"#, Flavor::Dos),
            (r#"find.exe /OFF needle file.txt"#, Flavor::Dos),
            (r#"find.exe /OFFLINE needle file.txt"#, Flavor::Dos),
        ] {
            assert_eq!(find_heuristic_impl(&wide(cmd)), flavor);
        }
    }
}
