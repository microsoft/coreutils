// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! NOTE: The code in this file is a mechanical translation of ulib's arg.cxx to a large extent.
//! I tried to clean it up as much as I had time, but it's still very ugly. Sorry.
//! The original (plus supporting code) is well over 5000 lines, hence the difficulty.

use core::mem::MaybeUninit;
use core::{ptr, slice};

use alloc::vec::Vec;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Environment::GetCommandLineW;

use crate::MSG_USAGE;
use crate::buffer::WideString;
use crate::io::{StatusResult, stderr_handle, write_stderr_str, write_stdout_str, write_to_handle};
use crate::path::{directory_exists, full_path, path_state, query_device_len};

unsafe extern "C" {
    pub fn wcslen(buf: *const u16) -> usize;
    pub fn wcsnlen(buf: *const u16, max: usize) -> usize;
}

pub struct FindState {
    pub pattern: WideString,
    pub case_sensitive: bool,
    pub lines_containing_pattern: bool,
    pub output_lines: bool,
    pub output_line_numbers: bool,
    pub skip_offline_files: bool,
    pub found_any: bool,
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            pattern: WideString::new(),
            case_sensitive: true,
            lines_containing_pattern: true,
            output_lines: true,
            output_line_numbers: false,
            skip_offline_files: true,
            found_any: false,
        }
    }
}

/// ARGUMENT_LEXEMIZER::PrepareToParse + DoParsing + MULTIPLE_PATH_ARGUMENT.
pub fn parse_command_line() -> StatusResult<(FindState, Vec<WideString>)> {
    // QUIRK / BUG: ulib has two `ARGUMENT_LEXEMIZER::PutSeparators` overloads.
    // The PCWSTRING one folds `_WhiteSpace` and `_SwitchChars` into `_SeparatorString`,
    // while the PCSTR one has those two lines commented out. find.cxx passes a narrow string.
    // So, the only reason '/' is not treated as a flag in "forward/slash/paths" is a coincidence.

    // Technically this could be trivially folded into the loop below.
    // I left it like it works in the original for now.
    let lexemes = prepare_to_parse()?;

    // ARGUMENT_LEXEMIZER::DoParsing, heavily modified & inlined.

    let mut state = FindState::default();
    let mut paths: Vec<WideString> = Vec::new();
    let mut wildcard_expansion_failed: Option<WideString> = None;
    let mut seen = SeenArgs::default();
    let mut lexemes = lexemes.into_iter();

    // find.cxx declares:
    //   ProgramNameArgument.Initialize("*")
    //   FlagCaseInsensitive.Initialize( "/I" )
    //   FlagNegativeSearch.Initialize( "/V" )
    //   FlagCountLines.Initialize( "/C" )
    //   FlagDisplayNumbers.Initialize( "/N" )
    //   FlagIncludeOfflineFiles.Initialize( "/OFFLINE" )
    //   FlagIncludeOfflineFiles2.Initialize( "/OFF" )
    //   FlagDisplayHelp.Initialize( "/?" )
    //   FlagInvalid.Initialize( "/*" )
    //   StringPattern.Initialize( "\"*\"" )
    //   _PathArguments.Initialize( "*", FALSE, TRUE )
    //
    // The loop below implements them in that particular order too.
    // I.e., this cannot be adapted to other ulib based utilities.

    // ProgramNameArgument
    _ = lexemes.next();

    loop {
        let Some(lexeme) = lexemes.next() else {
            break;
        };
        let lexeme = lexeme.as_slice();

        // Flag*
        if try_flag_argument(lexeme, &mut seen) {
            continue;
        }

        // FlagInvalid
        if try_invalid_switch_argument(lexeme, &mut seen) {
            break;
        }

        // StringPattern
        if try_string_pattern_argument(lexeme, &mut state, &mut seen) {
            continue;
        }

        // _PathArguments
        if set_path_argument(lexeme, &mut paths, &mut wildcard_expansion_failed) {
            // MULTIPLE_PATH_ARGUMENT resets _fValueSet, so it can match again.
            continue;
        }

        break;
    }

    // Post-parse semantics from find.cxx' Initialize().

    // aka: if not all arguments were consumed...
    if lexemes.next().is_some() {
        if seen.invalid_switch {
            write_stderr_str("FIND: Invalid switch\r\n");
            return Err(2);
        } else {
            write_stderr_str("FIND: Parameter format not correct\r\n");
            return Err(2);
        }
    }

    if let Some(failed_pattern) = wildcard_expansion_failed {
        let mut w = WideString::new();
        w.push_str("File not found - ");
        w.push_wide(&failed_pattern);
        w.push_str("\r\n");
        write_to_handle(stderr_handle(), &w);
        return Err(2);
    }

    if seen.help {
        write_stdout_str(MSG_USAGE);
        return Err(0);
    }

    if !seen.string_pattern {
        write_stderr_str("FIND: Parameter format not correct\r\n");
        return Err(2);
    }

    state.case_sensitive = !seen.case_insensitive;
    state.lines_containing_pattern = !seen.negative_search;
    state.output_lines = !seen.count_lines;
    state.output_line_numbers = seen.display_numbers;
    state.skip_offline_files = !seen.offline && !seen.off;

    Ok((state, paths))
}

fn raw_command_line() -> &'static [u16] {
    unsafe {
        let p = GetCommandLineW();
        let len = wcslen(p);
        slice::from_raw_parts(p, len)
    }
}

fn is_argument_separator(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16
}

fn is_argument_whitespace(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16
}

fn is_switch(c: u16) -> bool {
    c == b'/' as u16
}

enum Lexeme {
    Borrowed(&'static [u16]),
    Owned(WideString),
}

impl Lexeme {
    fn as_slice(&self) -> &[u16] {
        match self {
            Lexeme::Borrowed(s) => s,
            Lexeme::Owned(w) => w.as_slice(),
        }
    }
}

/// Contains logic from `ARGUMENT_LEXEMIZER::PrepareToParse`.
fn prepare_to_parse() -> StatusResult<Vec<Lexeme>> {
    let cmd_line = raw_command_line();
    let mut lexemes = Vec::new();
    let mut pos = 0;
    let n = cmd_line.len();

    while pos < n {
        while pos < n && is_argument_whitespace(cmd_line[pos]) {
            pos += 1;
        }
        if pos >= n {
            break;
        }

        let tok_start = pos;
        while pos < n {
            let ch = cmd_line[pos];

            if pos != tok_start && is_argument_separator(ch) {
                break;
            }

            if ch == b'"' as u16 {
                pos += 1;
                loop {
                    let close = cmd_line[pos..].iter().position(|&c| c == b'"' as u16);
                    match close {
                        Some(offset) => {
                            pos += offset + 1;
                            if pos < n && cmd_line[pos] == b'"' as u16 {
                                pos += 1;
                                continue;
                            }
                            break;
                        }
                        None => {
                            write_stderr_str("FIND: Parameter format not correct\r\n");
                            return Err(2);
                        }
                    }
                }
                continue;
            }

            pos += 1;
        }

        lexemes.push(collapse_quoted_lexeme(&cmd_line[tok_start..pos]));
    }

    Ok(lexemes)
}

fn collapse_quoted_lexeme(raw: &'static [u16]) -> Lexeme {
    let mut token: Option<WideString> = None;
    let mut i = 0;

    while i < raw.len() {
        let ch = raw[i];
        if let Some(token) = &mut token {
            token.push_char(ch);
        }

        if ch == b'"' as u16 {
            i += 1;
            while i < raw.len() {
                let qch = raw[i];
                if let Some(token) = &mut token {
                    token.push_char(qch);
                }

                if qch == b'"' as u16 {
                    if i + 1 < raw.len() && raw[i + 1] == b'"' as u16 {
                        if token.is_none() {
                            token = Some(WideString::from_slice(&raw[..=i]));
                        }
                        i += 2;
                        continue;
                    }

                    i += 1;
                    break;
                }

                i += 1;
            }
        } else {
            i += 1;
        }
    }

    match token {
        Some(token) => Lexeme::Owned(token),
        None => Lexeme::Borrowed(raw),
    }
}

#[derive(Default)]
struct SeenArgs {
    case_insensitive: bool,
    negative_search: bool,
    count_lines: bool,
    display_numbers: bool,
    offline: bool,
    off: bool,
    help: bool,
    invalid_switch: bool,
    string_pattern: bool,
}

fn try_flag_argument(lexeme: &[u16], seen: &mut SeenArgs) -> bool {
    if !lexeme.first().is_some_and(|&c| is_switch(c)) {
        return false;
    }

    let lex_body = &lexeme[1..];

    set_flag_if_match(lex_body, b"I", &mut seen.case_insensitive)
        || set_flag_if_match(lex_body, b"V", &mut seen.negative_search)
        || set_flag_if_match(lex_body, b"C", &mut seen.count_lines)
        || set_flag_if_match(lex_body, b"N", &mut seen.display_numbers)
        || set_flag_if_match(lex_body, b"OFFLINE", &mut seen.offline)
        || set_flag_if_match(lex_body, b"OFF", &mut seen.off)
        || set_flag_if_match(lex_body, b"?", &mut seen.help)
}

fn set_flag_if_match(lex_body: &[u16], pattern: &[u8], seen: &mut bool) -> bool {
    if !*seen
        && lex_body.len() == pattern.len()
        && lex_body.iter().zip(pattern.iter()).all(|(&l, &p)| {
            let l = if l >= b'a' as u16 && l <= b'z' as u16 {
                l - 0x20
            } else {
                l
            };
            l == p as u16
        })
    {
        *seen = true;
        true
    } else {
        false
    }
}

fn try_invalid_switch_argument(lexeme: &[u16], seen: &mut SeenArgs) -> bool {
    if lexeme.first().is_some_and(|&c| is_switch(c)) && !seen.invalid_switch {
        seen.invalid_switch = true;
        true
    } else {
        false
    }
}

fn try_string_pattern_argument(lexeme: &[u16], state: &mut FindState, seen: &mut SeenArgs) -> bool {
    if !seen.string_pattern
        && lexeme.len() >= 2
        && lexeme[0] == b'"' as u16
        && lexeme[lexeme.len() - 1] == b'"' as u16
    {
        state.pattern = WideString::from_slice(&lexeme[1..lexeme.len() - 1]);
        seen.string_pattern = true;
        true
    } else {
        false
    }
}

fn set_path_argument(
    lexeme: &[u16],
    paths: &mut Vec<WideString>,
    wildcard_expansion_failed: &mut Option<WideString>,
) -> bool {
    if lexeme.first().is_some_and(|&c| is_switch(c)) {
        return false;
    }

    let mut path = WideString::wrap(
        lexeme
            .iter()
            .copied()
            .filter(|&c| c != b'"' as u16)
            .collect(),
    );

    if path_has_name_wildcard(&path) {
        expand_wildcard_path(lexeme, &mut path, paths, wildcard_expansion_failed);
    } else {
        paths.push(path);
    }

    true
}

/// Contains the wildcard logic from `MULTIPLE_PATH_ARGUMENT::SetValue`.
/// The original ulib code implements a very complex globbing logic in user-code. In this version
/// I just pass it to FindFirstFileW. I failed to see why the original convoluted logic was necessary.
fn expand_wildcard_path(
    lexeme: &[u16],
    path: &mut WideString,
    paths: &mut Vec<WideString>,
    wildcard_expansion_failed: &mut Option<WideString>,
) {
    let Some(full_path) = full_path(path) else {
        return;
    };

    let full_state = path_state(&full_path);
    if full_state.prefix_with_separator_len >= full_path.len() {
        return;
    }

    let mut directory = WideString::from_slice(&full_path[..full_state.prefix_len]);
    if !directory_exists(&mut directory) {
        return;
    }

    path_append_base(
        &mut directory,
        &full_path[full_state.prefix_with_separator_len..],
    );

    let mut find_data = MaybeUninit::<WIN32_FIND_DATAW>::zeroed();
    let hfind = unsafe {
        FindFirstFileExW(
            directory.as_cstr().get(),
            FindExInfoBasic,
            find_data.as_mut_ptr() as *mut _,
            FindExSearchNameMatch,
            ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if hfind == INVALID_HANDLE_VALUE {
        if wildcard_expansion_failed.is_none() {
            *wildcard_expansion_failed = Some(WideString::from_slice(lexeme));
        }
        return;
    }

    let prefix = &path[..path_state(path).prefix_len];
    let mut any_match = false;

    loop {
        let fd = unsafe { find_data.assume_init_mut() };
        let name_len = unsafe { wcsnlen(fd.cFileName.as_ptr(), fd.cFileName.len()) };
        let name = &fd.cFileName[..name_len];

        if !name.starts_with(&[b'.' as u16]) && !name.starts_with(&[b'.' as u16, b'.' as u16]) {
            let mut path = WideString::from_slice(prefix);
            path_append_base(&mut path, name);
            paths.push(path);
            any_match = true;
        }

        if unsafe { FindNextFileW(hfind, fd as *mut WIN32_FIND_DATAW) } == 0 {
            break;
        }
    }
    unsafe { FindClose(hfind) };

    if !any_match && wildcard_expansion_failed.is_none() {
        *wildcard_expansion_failed = Some(WideString::from_slice(lexeme));
    }
}

fn path_has_name_wildcard(path: &[u16]) -> bool {
    path[path_state(path).prefix_len..]
        .iter()
        .any(|&c| c == b'*' as u16 || c == b'?' as u16)
}

// Roughly matches `PATH::AppendBase`.
fn path_append_base(path: &mut WideString, base: &[u16]) {
    if !path.is_empty()
        && !(path.len() == query_device_len(path) && path.last() == Some(&(b':' as u16)))
        && path.last() != Some(&(b'\\' as u16))
    {
        path.push_char(b'\\' as u16);
    }

    path.push_wide(base);
}
