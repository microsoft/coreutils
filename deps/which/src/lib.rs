// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write as _};
use std::path::{Component, Path};

use clap::{Arg, ArgAction, Command};
use uucore::Args;
use uucore::error::UResult;

pub fn uu_app() -> Command {
    Command::new("which")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Locate a command in PATH.")
        .override_usage("which [OPTION]... COMMAND...")
        .arg(
            Arg::new("all")
                .short('a')
                .long("all")
                .help("print all matching pathnames of each command")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("commands")
                .value_name("COMMAND")
                .num_args(1..)
                .value_parser(clap::value_parser!(OsString)),
        )
}

pub fn uumain(args: impl Args) -> i32 {
    match uumain_impl(args) {
        Ok(code) => code,
        Err(err) => {
            let code = err.code();
            _ = writeln!(io::stderr(), "which: {err}");
            code
        }
    }
}

fn uumain_impl(args: impl Args) -> UResult<i32> {
    let matches =
        uucore::clap_localization::handle_clap_result_with_exit_code(uu_app(), args, 255)?;
    let all = matches.get_flag("all");
    let commands = matches.get_many::<OsString>("commands").unwrap_or_default();

    let path_string = env::var_os("PATH").unwrap_or_default();
    let path = env::split_paths(&path_string).collect::<Vec<_>>();
    let pathext = env::var_os("PATHEXT").map(|mut pathext| {
        pathext.make_ascii_lowercase();
        pathext
    });
    let pathext = pathext
        .as_deref()
        .unwrap_or_else(|| OsStr::new(".com;.exe;.bat;.cmd"))
        .as_encoded_bytes()
        .split(|byte| *byte == b';')
        .filter(|entry| !entry.is_empty())
        // Semicolons are single-byte characters in the self-synchronizing OsStr encoding.
        .map(|entry| unsafe { OsStr::from_encoded_bytes_unchecked(entry) })
        .collect::<Vec<_>>();

    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut err = io::BufWriter::new(io::stderr().lock());
    let mut missing = 0;

    for command in commands {
        let mut components = Path::new(command).components();

        if let Some(Component::Normal(file_name)) = components.next_back()
            && let file_path = components.as_path()
            && !file_path.as_os_str().is_empty()
        {
            if !find_at(Path::new(command), &pathext, all, &mut out) {
                print_failure(&mut err, file_name, file_path.as_os_str());
                missing += 1;
            }
        } else {
            let mut found = false;
            for dir in &path {
                found |= find_at(&dir.join(command), &pathext, all, &mut out);
            }
            if !found {
                print_failure(&mut err, command, &path_string);
                missing += 1;
            }
        }
    }

    _ = out.flush()?;
    Ok(missing & 0xff as i32)
}

fn find_at(base: &Path, pathext: &[&OsStr], all: bool, out: &mut impl io::Write) -> bool {
    let mut found = false;

    if base.extension().is_some() {
        if has_pathext(&base, pathext) && base.is_file() {
            found = true;
            print_hit(out, &base);
        }
    } else {
        for ext in pathext {
            let mut candidate = base.to_path_buf();
            if !ext.as_encoded_bytes().starts_with(b".") {
                candidate.as_mut_os_string().push(".");
            }
            candidate.as_mut_os_string().push(ext);
            if candidate.is_file() {
                found = true;
                print_hit(out, &candidate);
                if !all {
                    break;
                }
            }
        }
    }

    found
}

fn has_pathext(path: &Path, pathext: &[&OsStr]) -> bool {
    let Some(extension) = path.extension() else {
        return false;
    };
    let extension = extension.as_encoded_bytes();
    pathext.iter().any(|candidate| {
        let candidate = candidate.as_encoded_bytes();
        let candidate = candidate.strip_prefix(b".").unwrap_or(candidate);
        candidate.eq_ignore_ascii_case(extension)
    })
}

fn print_hit(out: &mut impl io::Write, path: &Path) {
    _ = writeln!(out, "{}", path.display());
}

fn print_failure(err: &mut impl io::Write, name: &OsStr, path: &OsStr) {
    _ = writeln!(err, "which: no {} in ({})", name.display(), path.display());
}
