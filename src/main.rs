// Copyright (c) uutils developers, Microsoft Corporation.
// Licensed under the MIT License.
//
// NOTE: This file is derived from uutils/coreutils' original main.rs and includes
// Microsoft-authored changes, which Microsoft makes available to uutils
// under the uutils MIT License for upstream incorporation. See NOTICE.md.

mod manager;
mod nthelpers;

use std::borrow::Cow;
use std::cmp;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write as _, stderr};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;

use clap::Command;
use itertools::Itertools as _;
use uucore::Args;
use uucore::display::Quotable as _;
use uucore::windows_sys::Win32::System::Threading::GetCurrentProcess;
use uucore::{error::strip_errno, locale};
use windows_sys::Win32::Globalization::CP_UTF8;
use windows_sys::Win32::System::Console;
use windows_sys::Win32::System::Threading::TerminateProcess;

unsafe extern "C" {
    fn atexit(cb: extern "C" fn()) -> i32;
    unsafe fn ntsort_main(argc: i32, argv: *const *const u8) -> i32;
}

include!(concat!(env!("OUT_DIR"), "/uutils_map.rs"));

// While uucore::Args is a trait, this is the actual type it'll resolve to in the end.
type ArgsType = std::iter::Chain<std::vec::IntoIter<std::ffi::OsString>, wild::ArgsOs>;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const UTIL_MAP: UtilityMap<ArgsType> = util_map();

fn usage<T>(utils: &UtilityMap<T>, name: &str) {
    let display_list = utils.keys().copied().join(", ");
    let width = cmp::min(textwrap::termwidth(), 100) - 8; // (opinion/heuristic) max 100 chars wide with 4 character side indentions
    let indent_list = textwrap::indent(&textwrap::fill(&display_list, width), "    ");
    let common_core_string = "
Functions:
      '<coreutils>' [arguments...]

";
    let s = format!(
        "{name} {VERSION} (multi-call binary)

Usage: {name} [function [arguments...]]
       {name} --list

{common_core_string}Options:
      --list    lists all defined functions, one per row

Currently defined functions:

{indent_list}"
    );
    if let Err(e) = writeln!(io::stdout(), "{s}")
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        let _ = writeln!(io::stderr(), "coreutils: {}", strip_errno(&e));
        exit(1);
    }
}

fn main() {
    // NOTE: The stdlib checks the active CP and will call WriteFile directly if it's CP_UTF8.
    //
    // The good news is that this just so happens to not negatively affect ntfind,
    // because through ulib it incorrectly checks the input CP instead of the output one.
    // ntsort just hardcodes to CP_OEMCP, so it also isn't affected.
    set_console_modes();

    // `wild::args_os()` is what `uucore::args_os()` uses under the hood.
    // By using it directly we avoid duplicating all arg strings.
    let mut args = wild::args_os();

    let binary = binary_path(&mut args);
    let binary_as_util = name(&binary).unwrap_or_else(|| {
        usage(&UTIL_MAP, "<unknown binary name>");
        exit(0);
    });

    // binary name ends with util name?
    let is_coreutils = binary_as_util.ends_with("utils");
    let matched_util = UTIL_MAP
        .keys()
        .filter(|&&u| binary_as_util.ends_with(u) && !is_coreutils)
        .max_by_key(|u| u.len()); //Prefer stty more than tty. *utils is not ls

    let util_name = if let Some(&util) = matched_util {
        Some(OsString::from(util))
    } else if is_coreutils || binary_as_util.ends_with("box") {
        // todo: Remove support of "*box" from binary
        uucore::set_utility_is_second_arg();
        args.next()
    } else {
        not_found(&OsString::from(binary_as_util));
    };

    // 0th argument equals util name?
    if let Some(util_os) = util_name {
        let Some(util) = util_os.to_str() else {
            not_found(&util_os)
        };

        match util {
            "--list" => {
                // we should fail with additional args https://github.com/uutils/coreutils/issues/11383#issuecomment-4082564058
                if args.next().is_some() {
                    let _ = writeln!(io::stderr(), "coreutils: invalid argument");
                    exit(1);
                }
                let mut out = io::stdout().lock();
                for util in UTIL_MAP.keys() {
                    if let Err(e) = writeln!(out, "{util}")
                        && e.kind() != io::ErrorKind::BrokenPipe
                    {
                        let _ = writeln!(io::stderr(), "coreutils: {}", strip_errno(&e));
                        exit(1);
                    }
                }
                exit(0);
            }
            "--version" | "-V" => {
                if let Err(e) = writeln!(io::stdout(), "coreutils {VERSION} (multi-call binary)")
                    && e.kind() != io::ErrorKind::BrokenPipe
                {
                    let _ = writeln!(io::stderr(), "coreutils: {}", strip_errno(&e));
                    exit(1);
                }
                exit(0);
            }
            // Not a special command: fallthrough to calling a util
            _ => {}
        }

        match UTIL_MAP.get(util) {
            Some(&(uumain, _)) => {
                // TODO: plug the deactivation of the translation
                // and load the English strings directly at compilation time in the
                // binary to avoid the load of the flt
                // Could be something like:
                // #[cfg(not(feature = "only_english"))]
                setup_localization_or_exit(util);
                exit(uumain(vec![util_os].into_iter().chain(args)));
            }
            None => {
                if util == "--help" || util == "-h" {
                    // see if they want help on a specific util
                    if let Some(util_os) = args.next() {
                        let Some(util) = util_os.to_str() else {
                            not_found(&util_os)
                        };

                        match UTIL_MAP.get(util) {
                            Some(&(uumain, _)) => {
                                setup_localization_or_exit(util);
                                let code = uumain(
                                    vec![util_os, OsString::from("--help")]
                                        .into_iter()
                                        .chain(args),
                                );
                                io::stdout().flush().expect("could not flush stdout");
                                exit(code);
                            }
                            None => not_found(&util_os),
                        }
                    }
                    usage(&UTIL_MAP, binary_as_util);
                    exit(0);
                } else if util.starts_with('-') {
                    // Argument looks like an option but wasn't recognized
                    unrecognized_option(binary_as_util, &util_os);
                } else {
                    not_found(&util_os);
                }
            }
        }
    } else {
        // GNU just fails, but busybox tests needs usage
        // todo: patch the test suite instead
        if binary_as_util.ends_with("box") {
            usage(&UTIL_MAP, binary_as_util);
        } else {
            let _ = writeln!(io::stderr(), "coreutils: missing argument");
        }
        exit(1);
    }
}

fn binary_path(args: &mut impl Iterator<Item = OsString>) -> std::path::PathBuf {
    match args.next() {
        Some(ref s) if !s.is_empty() => PathBuf::from(s),
        // the fallback is valid only for hardlinks
        _ => std::env::current_exe().unwrap(),
    }
}

fn name(binary_path: &Path) -> Option<&str> {
    binary_path.file_stem()?.to_str()
}

fn not_found(util: &OsStr) -> ! {
    let _ = writeln!(
        stderr(),
        "coreutils: unknown program '{}'",
        util.maybe_quote()
    );
    exit(1);
}

fn unrecognized_option(binary_name: &str, option: &OsStr) -> ! {
    let _ = writeln!(
        stderr(),
        "{binary_name}: unrecognized option '{}'",
        option.to_string_lossy()
    );
    exit(1);
}

fn setup_localization_or_exit(util_name: &str) {
    let util_name = get_canonical_util_name(util_name);
    locale::setup_localization(util_name).unwrap_or_else(|err| {
        match err {
            locale::LocalizationError::ParseResource {
                error: err_msg,
                snippet,
            } => eprintln!("Localization parse error at {snippet}: {err_msg}"),
            other => eprintln!("Could not init the localization system: {other}"),
        }
        exit(99)
    });
}

fn get_canonical_util_name(util_name: &str) -> &str {
    match util_name {
        // uu_test aliases - '[' is an alias for test
        "[" => "test",
        "dir" => "ls",  // dir is an alias for ls
        "vdir" => "ls", // vdir is an alias for ls

        // Default case - return the util name as is
        _ => util_name,
    }
}

const EXPECTED_OUTPUT_MODE: u32 = Console::ENABLE_PROCESSED_OUTPUT
    | Console::ENABLE_WRAP_AT_EOL_OUTPUT
    | Console::ENABLE_VIRTUAL_TERMINAL_PROCESSING;

static ORIGINAL_OUTPUT_CP: AtomicU32 = AtomicU32::new(CP_UTF8);
static ORIGINAL_OUTPUT_MODE: AtomicU32 = AtomicU32::new(EXPECTED_OUTPUT_MODE);

/// Sets the console code page and output modes.
fn set_console_modes() {
    unsafe {
        let mut cp = Console::GetConsoleOutputCP();
        if cp == 0 {
            cp = CP_UTF8;
        }
        if cp != CP_UTF8 {
            Console::SetConsoleOutputCP(CP_UTF8);
        }

        let stdout = Console::GetStdHandle(Console::STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if Console::GetConsoleMode(stdout, &raw mut mode) == 0 {
            mode = EXPECTED_OUTPUT_MODE;
        }
        if mode != EXPECTED_OUTPUT_MODE {
            Console::SetConsoleMode(stdout, EXPECTED_OUTPUT_MODE);
        }

        ORIGINAL_OUTPUT_CP.store(cp, std::sync::atomic::Ordering::Relaxed);
        ORIGINAL_OUTPUT_MODE.store(mode, std::sync::atomic::Ordering::Relaxed);

        atexit(restore_console_modes);
    }
}

extern "C" fn restore_console_modes() {
    unsafe {
        let cp = ORIGINAL_OUTPUT_CP.load(std::sync::atomic::Ordering::Relaxed);
        let mode = ORIGINAL_OUTPUT_MODE.load(std::sync::atomic::Ordering::Relaxed);

        if cp != CP_UTF8 {
            Console::SetConsoleOutputCP(cp);
        }

        if mode != EXPECTED_OUTPUT_MODE {
            let stdout = Console::GetStdHandle(Console::STD_OUTPUT_HANDLE);
            Console::SetConsoleMode(stdout, mode);
        }
    }
}

/// Restores the console modes and then exits.
///
/// NOTE: Regular uutils/coreutils calls std::process::exit, which flushes stdout buffers.
/// However, the utils are designed such that they can be used in-proc so
/// if they didn't flush themselves, they would be considered buggy anyway.
///
/// TerminateProcess is used because it is technically superior to ExitProcess.
fn exit(code: i32) -> ! {
    unsafe {
        restore_console_modes();
        TerminateProcess(GetCurrentProcess(), code as u32);
        std::hint::unreachable_unchecked();
    }
}

fn find_uumain<T: Args>(args: T) -> i32 {
    if nthelpers::is_ntfind_invocation() {
        ntfind::ntfind_main()
    } else {
        let strs: Vec<_> = findutils_collect_args(args);
        let deps = findutils::find::StandardDependencies::new();
        findutils::find::find_main(&strs, &deps)
    }
}

fn find_uu_app() -> Command {
    unreachable!()
}

fn xargs_uumain<T: Args>(args: T) -> i32 {
    let strs: Vec<_> = findutils_collect_args(args);
    findutils::xargs::xargs_main(&strs)
}

fn xargs_uu_app() -> Command {
    unreachable!()
}

fn findutils_collect_args<T: Args>(args: T) -> Vec<&'static str> {
    args.map(|s| {
        let bytes = s.into_encoded_bytes();

        // `String::from_utf8_lossy_owned` is an unstable feature. Here's a copy.
        let string = if let Cow::Owned(string) = String::from_utf8_lossy(&bytes) {
            string
        } else {
            unsafe { String::from_utf8_unchecked(bytes) }
        };

        // findutils expects `&[&str]` so we leak the `String` here. This is because
        // we have a one-shot lifecycle. There's no point in doing memory management.
        &*string.leak()
    })
    .collect()
}

fn sort_uumain<T: Args>(args: T) -> i32 {
    let mut args: Vec<OsString> = args.collect();
    if nthelpers::is_ntsort_invocation(&args) {
        for arg in &mut args {
            arg.push("\0");
        }
        let ptrs: Vec<_> = args.iter().map(|v| v.as_encoded_bytes().as_ptr()).collect();

        unsafe {
            // NOTE: ntsort calls exit() on /? and on error.
            atexit(restore_console_modes);
            ntsort_main(ptrs.len() as i32, ptrs.as_ptr())
        }
    } else {
        sort::uumain(args.into_iter())
    }
}

fn sort_uu_app() -> Command {
    sort::uu_app()
}
