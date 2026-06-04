// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, Command};
use uucore::Args;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn uumain(args: impl Args) -> i32 {
    match uumain_impl(args) {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(io::stderr(), "which: {err}");
            1
        }
    }
}

fn uumain_impl(args: impl Args) -> Result<i32, String> {
    let matches = match uu_app().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(err) => {
            let _ = err.print();
            return Ok(if err.use_stderr() { 1 } else { 0 });
        }
    };

    let all = matches.get_flag("all");
    let Some(commands) = matches.get_many::<OsString>("commands") else {
        return Err("missing command operand".to_string());
    };

    let mut all_found = true;
    for command in commands {
        let hits = find_command(command, all);
        if hits.is_empty() {
            all_found = false;
            continue;
        }

        for hit in hits {
            println!("{}", hit.display());
        }
    }

    Ok(if all_found { 0 } else { 1 })
}

pub fn uu_app() -> Command {
    Command::new("which")
        .version(VERSION)
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

fn find_command(command: &OsStr, all: bool) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    let pathext = pathext();

    let mut push_hit = |path: PathBuf| {
        let key = normalize_seen_key(&path);
        if seen.insert(key) {
            hits.push(path);
        }
    };

    if has_path_separator(Path::new(command)) {
        for candidate in candidates(PathBuf::from(command), &pathext) {
            if is_regular_file(&candidate) {
                push_hit(candidate);
                if !all {
                    return hits;
                }
            }
        }
        return hits;
    }

    let Some(path_var) = env::var_os("PATH") else {
        return hits;
    };

    for dir in env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }

        let base = dir.join(command);
        for candidate in candidates(base, &pathext) {
            if is_regular_file(&candidate) {
                push_hit(candidate);
                if !all {
                    return hits;
                }
            }
        }
    }

    hits
}

fn candidates(base: PathBuf, pathext: &[OsString]) -> Vec<PathBuf> {
    let mut out = vec![base.clone()];
    if base.extension().is_some() {
        return out;
    }

    for ext in pathext {
        let mut candidate = base.clone();
        candidate.as_mut_os_string().push(ext);
        out.push(candidate);
    }

    out
}

fn pathext() -> Vec<OsString> {
    let Some(value) = env::var_os("PATHEXT") else {
        return default_pathext();
    };

    let mut out = Vec::new();
    for ext in value.to_string_lossy().split(';') {
        if ext.is_empty() {
            continue;
        }

        let ext = if ext.starts_with('.') {
            ext.to_string()
        } else {
            format!(".{ext}")
        };
        out.push(OsString::from(ext));
    }

    if out.is_empty() {
        default_pathext()
    } else {
        out
    }
}

fn default_pathext() -> Vec<OsString> {
    [".COM", ".EXE", ".BAT", ".CMD"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

fn has_path_separator(path: &Path) -> bool {
    let path = path.as_os_str().to_string_lossy();
    path.contains('/') || path.contains('\\')
}

fn is_regular_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.is_file())
}

fn normalize_seen_key(path: &Path) -> String {
    path.as_os_str().to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        old: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl Into<OsString>) -> Self {
            let old = env::var_os(key);
            unsafe {
                env::set_var(key, value.into());
            }
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(old) = &self.old {
                    env::set_var(self.key, old);
                } else {
                    env::remove_var(self.key);
                }
            }
        }
    }

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("ntwhich-test-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn finds_first_match_with_pathext() {
        let _lock = TEST_LOCK.lock().unwrap();
        let dir = temp_dir();
        fs::write(dir.join("tool.EXE"), []).unwrap();
        let _path = EnvGuard::set("PATH", dir.as_os_str());
        let _pathext = EnvGuard::set("PATHEXT", ".EXE");

        let hits = find_command(OsStr::new("tool"), false);

        assert_eq!(hits, vec![dir.join("tool.EXE")]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn all_returns_all_matches() {
        let _lock = TEST_LOCK.lock().unwrap();
        let dir1 = temp_dir();
        let dir2 = temp_dir();
        fs::write(dir1.join("tool.CMD"), []).unwrap();
        fs::write(dir2.join("tool.CMD"), []).unwrap();
        let joined = env::join_paths([dir1.as_os_str(), dir2.as_os_str()]).unwrap();
        let _path = EnvGuard::set("PATH", joined);
        let _pathext = EnvGuard::set("PATHEXT", ".CMD");

        let hits = find_command(OsStr::new("tool"), true);

        assert_eq!(hits, vec![dir1.join("tool.CMD"), dir2.join("tool.CMD")]);
        fs::remove_dir_all(dir1).unwrap();
        fs::remove_dir_all(dir2).unwrap();
    }

    #[test]
    fn searches_explicit_relative_path() {
        let _lock = TEST_LOCK.lock().unwrap();
        let dir = temp_dir();
        let old_dir = env::current_dir().unwrap();
        fs::create_dir(dir.join("bin")).unwrap();
        fs::write(dir.join("bin").join("tool.EXE"), []).unwrap();
        let _pathext = EnvGuard::set("PATHEXT", ".EXE");
        env::set_current_dir(&dir).unwrap();

        let hits = find_command(OsStr::new("bin/tool"), false);

        env::set_current_dir(old_dir).unwrap();
        assert_eq!(hits, vec![PathBuf::from("bin/tool.EXE")]);
        fs::remove_dir_all(dir).unwrap();
    }
}
