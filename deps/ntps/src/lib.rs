// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsString;
use std::io::{self, Write};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStringExt as _;
use std::collections::BTreeSet;

use clap::{Arg, ArgAction, Command};
use uucore::Args;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

pub fn uumain<T: Args>(args: T) -> i32 {
    let matches = match uu_app().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(err) => {
            let _ = err.print();
            return err.exit_code();
        }
    };

    let options = match Options::from_matches(&matches) {
        Ok(options) => options,
        Err(err) => {
            let _ = writeln!(io::stderr(), "ps: {err}");
            return 1;
        }
    };

    match list_processes() {
        Ok(processes) => {
            let processes = filter_processes(&processes, &options);
            print_processes(&processes, &options)
        }
        Err(err) => {
            let _ = writeln!(io::stderr(), "ps: {err}");
            1
        }
    }
}

pub fn uu_app() -> Command {
    Command::new("ps")
        .version(env!("CARGO_PKG_VERSION"))
        .about("List running processes")
        .arg(
            Arg::new("all")
                .short('A')
                .action(ArgAction::SetTrue)
                .help("List all processes"),
        )
        .arg(
            Arg::new("every")
                .short('e')
                .action(ArgAction::SetTrue)
                .help("List all processes"),
        )
        .arg(
            Arg::new("pid")
                .short('p')
                .long("pid")
                .num_args(1)
                .action(ArgAction::Append)
                .value_name("PID[,PID...]")
                .help("Select processes by process ID"),
        )
        .arg(
            Arg::new("ppid")
                .long("ppid")
                .num_args(1)
                .action(ArgAction::Append)
                .value_name("PID[,PID...]")
                .help("Select processes by parent process ID"),
        )
        .arg(
            Arg::new("format")
                .short('o')
                .long("format")
                .num_args(1)
                .action(ArgAction::Append)
                .value_name("FORMAT")
                .help("Select output columns: pid, ppid, comm, command, nlwp, pri"),
        )
        .arg(
            Arg::new("no-headers")
                .long("no-headers")
                .action(ArgAction::SetTrue)
                .help("Print no header line"),
        )
        .arg(
            Arg::new("bsd")
                .value_parser(["aux", "ax", "x"])
                .hide(true),
        )
}

#[derive(Debug, Eq, PartialEq)]
struct ProcessInfo {
    pid: u32,
    ppid: u32,
    threads: u32,
    priority: i32,
    name: String,
}

#[derive(Debug, Eq, PartialEq)]
struct Options {
    pids: BTreeSet<u32>,
    ppids: BTreeSet<u32>,
    columns: Vec<Column>,
    no_headers: bool,
}

impl Options {
    fn from_matches(matches: &clap::ArgMatches) -> Result<Self, String> {
        let bsd = matches.get_one::<String>("bsd").is_some();
        let columns = if let Some(values) = matches.get_many::<String>("format") {
            parse_columns(values)?
        } else if bsd {
            vec![
                Column::Pid,
                Column::Ppid,
                Column::Threads,
                Column::Priority,
                Column::Command,
            ]
        } else {
            vec![Column::Pid, Column::Ppid, Column::Command]
        };

        Ok(Self {
            pids: parse_id_list(matches.get_many::<String>("pid"), "pid")?,
            ppids: parse_id_list(matches.get_many::<String>("ppid"), "ppid")?,
            columns,
            no_headers: matches.get_flag("no-headers"),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Column {
    Pid,
    Ppid,
    Command,
    Threads,
    Priority,
}

impl Column {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "pid" => Some(Self::Pid),
            "ppid" => Some(Self::Ppid),
            "comm" | "command" | "args" => Some(Self::Command),
            "nlwp" | "thcount" | "threads" => Some(Self::Threads),
            "pri" | "priority" => Some(Self::Priority),
            _ => None,
        }
    }

    fn header(self) -> &'static str {
        match self {
            Self::Pid => "PID",
            Self::Ppid => "PPID",
            Self::Command => "COMMAND",
            Self::Threads => "NLWP",
            Self::Priority => "PRI",
        }
    }

    fn width(self) -> usize {
        match self {
            Self::Pid | Self::Ppid => 8,
            Self::Threads => 6,
            Self::Priority => 4,
            Self::Command => 0,
        }
    }

    fn value(self, process: &ProcessInfo) -> String {
        match self {
            Self::Pid => process.pid.to_string(),
            Self::Ppid => process.ppid.to_string(),
            Self::Command => process.name.clone(),
            Self::Threads => process.threads.to_string(),
            Self::Priority => process.priority.to_string(),
        }
    }
}

fn list_processes() -> io::Result<Vec<ProcessInfo>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;

    let mut processes = Vec::new();

    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) != 0 };
    while ok {
        processes.push(ProcessInfo {
            pid: entry.th32ProcessID,
            ppid: entry.th32ParentProcessID,
            threads: entry.cntThreads,
            priority: entry.pcPriClassBase,
            name: process_name(&entry.szExeFile),
        });
        ok = unsafe { Process32NextW(snapshot, &mut entry) != 0 };
    }

    unsafe {
        CloseHandle(snapshot);
    }

    processes.sort_by_key(|process| process.pid);
    Ok(processes)
}

fn filter_processes<'a>(processes: &'a [ProcessInfo], options: &Options) -> Vec<&'a ProcessInfo> {
    processes
        .iter()
        .filter(|process| options.pids.is_empty() || options.pids.contains(&process.pid))
        .filter(|process| options.ppids.is_empty() || options.ppids.contains(&process.ppid))
        .collect()
}

fn print_processes(processes: &[&ProcessInfo], options: &Options) -> i32 {
    let mut out = io::stdout().lock();

    if !options.no_headers && print_row(&mut out, options.columns.iter().map(|column| column.header()), &options.columns).is_err() {
            return 1;
    }

    for process in processes {
        let values = options.columns.iter().map(|column| column.value(process));
        if print_row(&mut out, values, &options.columns).is_err() {
            return 1;
        }
    }

    0
}

fn print_row<'a>(
    out: &mut impl Write,
    values: impl Iterator<Item = impl AsRef<str>>,
    columns: &'a [Column],
) -> io::Result<()> {
    for (idx, (value, column)) in values.zip(columns).enumerate() {
        if idx + 1 == columns.len() || column.width() == 0 {
            write!(out, "{}", value.as_ref())?;
        } else {
            write!(out, "{:>width$} ", value.as_ref(), width = column.width())?;
        }
    }

    writeln!(out)
}

fn parse_columns<'a>(values: impl Iterator<Item = &'a String>) -> Result<Vec<Column>, String> {
    let mut columns = Vec::new();

    for value in values {
        for name in value.split(',') {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }

            let Some(column) = Column::parse(&name) else {
                return Err(format!("unsupported output column '{name}'"));
            };

            columns.push(column);
        }
    }

    if columns.is_empty() {
        return Err("empty output format".into());
    }

    Ok(columns)
}

fn parse_id_list<'a>(
    values: Option<impl Iterator<Item = &'a String>>,
    option: &str,
) -> Result<BTreeSet<u32>, String> {
    let mut ids = BTreeSet::new();

    let Some(values) = values else {
        return Ok(ids);
    };

    for value in values {
        for id in value.split(',') {
            let id = id.trim();
            if id.is_empty() {
                return Err(format!("empty {option} value"));
            }

            let id = id
                .parse::<u32>()
                .map_err(|_| format!("invalid {option} value '{id}'"))?;
            ids.insert(id);
        }
    }

    Ok(ids)
}

fn process_name(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|&ch| ch == 0).unwrap_or(buffer.len());
    OsString::from_wide(&buffer[..len])
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_process_name_at_nul() {
        let mut buffer = [0u16; 260];
        let name: Vec<u16> = "example.exe".encode_utf16().collect();
        buffer[..name.len()].copy_from_slice(&name);

        assert_eq!(process_name(&buffer), "example.exe");
    }

    #[test]
    fn accepts_common_all_process_flags() {
        assert!(uu_app().try_get_matches_from(["ps", "-A"]).is_ok());
        assert!(uu_app().try_get_matches_from(["ps", "-e"]).is_ok());
        assert!(uu_app().try_get_matches_from(["ps", "aux"]).is_ok());
        assert!(uu_app().try_get_matches_from(["ps", "ax"]).is_ok());
    }

    #[test]
    fn parses_pid_filters() {
        let matches = uu_app()
            .try_get_matches_from(["ps", "-p", "1,2", "--ppid", "3"])
            .unwrap();
        let options = Options::from_matches(&matches).unwrap();

        assert!(options.pids.contains(&1));
        assert!(options.pids.contains(&2));
        assert!(options.ppids.contains(&3));
    }

    #[test]
    fn parses_custom_columns() {
        let matches = uu_app()
            .try_get_matches_from(["ps", "-o", "pid,ppid,comm,nlwp,pri"])
            .unwrap();
        let options = Options::from_matches(&matches).unwrap();

        assert_eq!(
            options.columns,
            vec![
                Column::Pid,
                Column::Ppid,
                Column::Command,
                Column::Threads,
                Column::Priority
            ]
        );
    }

    #[test]
    fn rejects_unknown_columns() {
        let matches = uu_app()
            .try_get_matches_from(["ps", "-o", "pid,user"])
            .unwrap();

        assert!(Options::from_matches(&matches).is_err());
    }

    #[test]
    fn filters_by_pid_and_ppid() {
        let processes = vec![
            ProcessInfo {
                pid: 1,
                ppid: 0,
                threads: 1,
                priority: 8,
                name: "init.exe".into(),
            },
            ProcessInfo {
                pid: 2,
                ppid: 1,
                threads: 2,
                priority: 8,
                name: "child.exe".into(),
            },
        ];
        let matches = uu_app()
            .try_get_matches_from(["ps", "-p", "2", "--ppid", "1"])
            .unwrap();
        let options = Options::from_matches(&matches).unwrap();

        let filtered = filter_processes(&processes, &options);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "child.exe");
    }
}
