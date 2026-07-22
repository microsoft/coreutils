// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// TODO: It would be cleaner to implement the entire PowerShell logic here in Rust.
// FWIW we lose 2 things this way:
// * Access to `$PROFILE.CurrentUserCurrentHost`
// * .NET's text decoding/encoding logic (including support for ACP encoding)

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process;
use std::ptr;

use clap::builder::{NonEmptyStringValueParser, PathBufValueParser};
use clap::{Arg, ArgMatches, Command};
use uucore::Args;
use uucore::error::{FromIo as _, UError, UResult, USimpleError};
use windows_sys::Win32::Foundation;
use windows_sys::Win32::Security::Cryptography;
use windows_sys::Win32::Storage::FileSystem;
use windows_sys::Win32::System::Console;
use windows_sys::Win32::System::Pipes;
use windows_sys::Win32::System::Registry;
use windows_sys::Win32::UI::Shell;
use windows_sys::Win32::UI::WindowsAndMessaging;
use windows_sys::w;

const REG_PATH: *const u16 = w!(r"SOFTWARE\Microsoft\coreutils");
const REG_DISABLED_UTILITIES: *const u16 = w!("DisabledUtilities");

#[uucore::main(no_signals)]
pub fn uumain<T: Args>(args: T) -> UResult<()> {
    let matches = uucore::clap_localization::handle_clap_result_with_exit_code(uu_app(), args, 2)?;

    // Redirect our stdout/stderr into the given named pipe if needed.
    // This is used for self-elevation.
    let _stdout_pipe = if let Some(path) = matches.get_one::<PathBuf>("stdout-pipe") {
        Some(connect_stdout_pipe(path)?)
    } else {
        None
    };

    let utilities = utility_names();
    let mut disabled = read_disabled_utilities()?;

    match matches.subcommand() {
        Some((action @ ("enable" | "disable"), matches)) => {
            let requested: BTreeSet<String> = matches
                .get_many::<String>("utilities")
                .expect("utility is a required parameter")
                .cloned()
                .collect();

            // Update the disabled set
            let remove = action == "enable";
            let mut unknown = Vec::new();
            for name in &requested {
                if !utilities.contains(name.as_str()) {
                    unknown.push(name.clone());
                    continue;
                }
                if remove {
                    disabled.remove(name);
                } else {
                    disabled.insert(name.clone());
                }
            }
            if !unknown.is_empty() {
                return Err(USimpleError::new(
                    1,
                    format!("unknown coreutils utility: {}", unknown.join(", ")),
                ));
            }

            if !ensure_elevated(matches, action, &requested)? {
                return Ok(());
            }

            write_disabled_utilities(&disabled)?;
            sync_install("refresh", &utilities, &disabled)
        }
        Some(("refresh", _)) => sync_install("", &utilities, &disabled),
        Some(("status", _)) => {
            for utility_name in utilities {
                let status = if disabled.contains(utility_name) {
                    "disabled"
                } else {
                    "enabled"
                };
                println!("{utility_name:16}{status}");
            }
            Ok(())
        }
        _ => unreachable!("clap enforces a known subcommand"),
    }
}

pub fn uu_app() -> Command {
    Command::new("coreutils-manager")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Manage coreutils utilities and PowerShell profiles")
        .arg(
            Arg::new("stdout-pipe")
                .long("stdout-pipe")
                .hide(true)
                .global(true)
                .value_parser(PathBufValueParser::new()),
        )
        .arg(
            Arg::new("no-elevate")
                .long("no-elevate")
                .hide(true)
                .global(true)
                .action(clap::ArgAction::SetTrue),
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("enable")
                .about("Enable one or more utilities")
                .arg(
                    Arg::new("utilities")
                        .help("Utility names to enable")
                        .num_args(1..)
                        .required(true)
                        .trailing_var_arg(true)
                        .value_parser(NonEmptyStringValueParser::new()),
                ),
        )
        .subcommand(
            Command::new("disable")
                .about("Disable one or more utilities")
                .arg(
                    Arg::new("utilities")
                        .help("Utility names to disable")
                        .num_args(1..)
                        .required(true)
                        .trailing_var_arg(true)
                        .value_parser(NonEmptyStringValueParser::new()),
                ),
        )
        .subcommand(Command::new("refresh").hide(true))
        .subcommand(Command::new("status").about("List all utilities with their status"))
}

fn utility_names() -> BTreeSet<&'static str> {
    super::UTIL_MAP
        .keys()
        .copied()
        .filter(|&n| n != "[" && n != "coreutils-manager")
        .collect()
}

fn connect_stdout_pipe(path: &Path) -> UResult<OwnedHandle> {
    let path = wide_null(path);
    let handle = unsafe {
        FileSystem::CreateFileW(
            path.as_ptr(),
            FileSystem::FILE_GENERIC_WRITE,
            0,
            ptr::null(),
            FileSystem::OPEN_EXISTING,
            FileSystem::FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == Foundation::INVALID_HANDLE_VALUE {
        return Err(last_os_error("failed to connect stdout pipe"));
    }

    if unsafe { Console::SetStdHandle(Console::STD_OUTPUT_HANDLE, handle) } == 0
        || unsafe { Console::SetStdHandle(Console::STD_ERROR_HANDLE, handle) } == 0
    {
        unsafe {
            Foundation::CloseHandle(handle);
        }
        return Err(last_os_error("failed to redirect stdout/stderr"));
    }

    Ok(OwnedHandle(handle))
}

fn ensure_elevated(
    matches: &ArgMatches,
    action: &str,
    utilities: &BTreeSet<String>,
) -> UResult<bool> {
    if unsafe { Shell::IsUserAnAdmin() != 0 } {
        Ok(true)
    } else {
        if matches.get_flag("no-elevate") {
            return Err(USimpleError::new(
                1,
                "administrator privileges are required".to_string(),
            ));
        }
        elevate(action, utilities)?;
        Ok(false)
    }
}

fn elevate(action: &str, utilities: &BTreeSet<String>) -> UResult<()> {
    let (pipe_path, pipe) = NamedPipe::with_random_pipe_path("coreutils-manager")?;
    let exe = std::env::current_exe()?;

    let mut command_line = OsString::new();
    if !exe
        .file_stem()
        .is_some_and(|stem| stem == "coreutils-manager")
    {
        command_line.push("coreutils-manager ");
    }
    _ = write!(command_line, "{action} --no-elevate --stdout-pipe ");
    command_line.push(pipe_path);
    for name in utilities {
        _ = write!(command_line, " {name}");
    }

    let exe = wide_null(&exe);
    let parameters = wide_null(&command_line);
    let result = unsafe {
        Shell::ShellExecuteW(
            ptr::null_mut(),
            w!("runas"),
            exe.as_ptr(),
            parameters.as_ptr(),
            ptr::null(),
            WindowsAndMessaging::SW_HIDE,
        )
    };
    if result as usize <= 32 {
        return Err(last_os_error("failed to elevate coreutils-manager"));
    }

    pipe.connect()?;
    pipe.copy_to_stdout()
}

fn sync_install(
    pwsh_install_action: &str,
    utility_names: &BTreeSet<&str>,
    disabled: &BTreeSet<String>,
) -> UResult<()> {
    let app_dir = app_dir()?;
    let bin_dir = app_dir.join("bin");
    let cmd_dir = app_dir.join("cmd");
    let coreutils_exe = app_dir.join("coreutils.exe");

    // Synchronize hardlinks
    {
        sync_hardlinks(utility_names, disabled, &coreutils_exe, &bin_dir, "exe")?;
        sync_hardlinks(utility_names, disabled, &coreutils_exe, &cmd_dir, "cmd")?;
        sync_hardlink(
            "coreutils-manager",
            disabled,
            &coreutils_exe,
            &bin_dir,
            "exe",
        )?;
    }

    // Refresh PowerShell profiles
    if !pwsh_install_action.is_empty() {
        let script = app_dir.join("pwsh-install.ps1");
        if !script.is_file() {
            return Ok(());
        }

        let status = match process::Command::new("pwsh.exe")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .arg("-Action")
            .arg(pwsh_install_action)
            .arg("-CmdDir")
            .arg(cmd_dir)
            .status()
        {
            Ok(status) => status,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(USimpleError::new(
                    1,
                    format!("failed to start pwsh.exe: {err}"),
                ));
            }
        };
        if !status.success() {
            return Err(USimpleError::new(
                1,
                format!("failed to refresh PowerShell profiles: {status}"),
            ));
        }
    }

    Ok(())
}

fn app_dir() -> UResult<PathBuf> {
    let exe = std::env::current_exe()?;
    // Navigate from...
    // * C:\Program Files\coreutils\bin\utility.exe
    // * C:\Program Files\coreutils\bin  (check if the suffix is bin/cmd)
    // * C:\Program Files\coreutils
    // Or from...
    // * C:\Program Files\coreutils\corutils.exe
    // * C:\Program Files\coreutils
    exe.parent()
        .and_then(|p| {
            if matches!(p.file_name(), Some(name) if name == "bin" || name == "cmd") {
                p.parent()
            } else {
                Some(p)
            }
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            USimpleError::new(
                1,
                format!("cannot determine install directory from {}", exe.display()),
            )
        })
}

fn sync_hardlinks(
    utility_names: &BTreeSet<&str>,
    disabled: &BTreeSet<String>,
    coreutils_path: &Path,
    dest_dir: &Path,
    suffix: &str,
) -> UResult<()> {
    fs::create_dir_all(dest_dir)?;
    for &utility_name in utility_names {
        sync_hardlink(utility_name, disabled, coreutils_path, dest_dir, suffix)?;
    }
    Ok(())
}

fn sync_hardlink(
    utility_name: &str,
    disabled: &BTreeSet<String>,
    coreutils_path: &Path,
    dest_dir: &Path,
    suffix: &str,
) -> UResult<()> {
    let link = dest_dir.join(format!("{utility_name}.{suffix}"));
    let (res, kind_ok) = if disabled.contains(utility_name) {
        (fs::remove_file(&link), io::ErrorKind::NotFound)
    } else {
        (
            fs::hard_link(coreutils_path, &link),
            io::ErrorKind::AlreadyExists,
        )
    };
    if let Err(err) = res
        && err.kind() != kind_ok
    {
        return Err(err.map_err_context(|| "failed to synchronize utility links".to_string()));
    }
    Ok(())
}

fn read_disabled_utilities() -> UResult<BTreeSet<String>> {
    OwnedHKEY::get_multi_sz_value(
        Registry::HKEY_LOCAL_MACHINE,
        REG_PATH,
        REG_DISABLED_UTILITIES,
    )
    .map(|data| parse_multi_sz(&data))
}

fn write_disabled_utilities(disabled: &BTreeSet<String>) -> UResult<()> {
    let key = OwnedHKEY::create_key(Registry::HKEY_LOCAL_MACHINE, REG_PATH)?;
    if disabled.is_empty() {
        let ret = unsafe { Registry::RegDeleteValueW(key.get(), REG_DISABLED_UTILITIES) };
        if ret != 0 && ret != Foundation::ERROR_FILE_NOT_FOUND {
            return Err(last_os_error(
                "failed to clear disabled aliases from registry",
            ));
        }
        Ok(())
    } else {
        let data = make_multi_sz(disabled.iter());
        reg_check_result("failed to write disabled aliases to registry", unsafe {
            Registry::RegSetValueExW(
                key.get(),
                REG_DISABLED_UTILITIES,
                0,
                Registry::REG_MULTI_SZ,
                data.as_ptr().cast(),
                (data.len() * size_of::<u16>()) as u32,
            )
        })?;
        Ok(())
    }
}

fn parse_multi_sz(data: &[u16]) -> BTreeSet<String> {
    data.split(|&ch| ch == 0)
        .filter(|&slice| !slice.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

fn make_multi_sz(values: impl Iterator<Item = impl AsRef<OsStr>>) -> Vec<u16> {
    let mut data = Vec::new();
    for value in values {
        data.extend(value.as_ref().encode_wide());
        data.push(0);
    }
    data.push(0);
    data
}

struct OwnedHandle(Foundation::HANDLE);

impl OwnedHandle {
    fn get(&self) -> Foundation::HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != Foundation::INVALID_HANDLE_VALUE {
            unsafe { Foundation::CloseHandle(self.0) };
        }
    }
}

struct OwnedHKEY(Registry::HKEY);

impl OwnedHKEY {
    fn create_key(key: Registry::HKEY, subkey: *const u16) -> UResult<Self> {
        let mut result = ptr::null_mut();
        reg_check_result("failed to create registry key", unsafe {
            Registry::RegCreateKeyW(key, subkey, &mut result)
        })?;
        Ok(Self(result))
    }

    fn get_multi_sz_value(
        key: Registry::HKEY,
        subkey: *const u16,
        value: *const u16,
    ) -> UResult<Vec<u16>> {
        let mut bytes = 0u32;
        let ret = unsafe {
            Registry::RegGetValueW(
                key,
                subkey,
                value,
                Registry::RRF_RT_REG_MULTI_SZ,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut bytes,
            )
        };
        if ret == Foundation::ERROR_FILE_NOT_FOUND || ret == Foundation::ERROR_PATH_NOT_FOUND {
            return Ok(Vec::new());
        }
        reg_check_result("failed to read from registry", ret)?;
        if bytes == 0 {
            return Ok(Vec::new());
        }

        let mut data: Vec<u16> = Vec::with_capacity(bytes as usize / 2 + 128);
        bytes = data.capacity() as u32 * 2;

        reg_check_result("failed to read from registry", unsafe {
            Registry::RegGetValueW(
                key,
                subkey,
                value,
                Registry::RRF_RT_REG_MULTI_SZ,
                ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut bytes,
            )
        })?;

        unsafe { data.set_len(bytes as usize / 2) };
        Ok(data)
    }

    fn get(&self) -> Registry::HKEY {
        self.0
    }
}

impl Drop for OwnedHKEY {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { Registry::RegCloseKey(self.0) };
        }
    }
}

struct NamedPipe(OwnedHandle);

impl NamedPipe {
    fn with_random_pipe_path(prefix: &str) -> UResult<(PathBuf, Self)> {
        let mut random = [0u8; 16];
        unsafe { Cryptography::ProcessPrng(random.as_mut_ptr(), random.len()) };

        let mut path = String::with_capacity(random.len() * 2 + prefix.len() + 10);
        _ = write!(path, "\\\\.\\pipe\\{prefix}-");
        for byte in random {
            _ = write!(path, "{byte:02x}");
        }

        let path = PathBuf::from(path);
        let pipe = Self::create(&path)?;
        Ok((path, pipe))
    }

    fn create(path: &Path) -> UResult<Self> {
        let path = wide_null(path);
        let handle = unsafe {
            Pipes::CreateNamedPipeW(
                path.as_ptr(),
                FileSystem::PIPE_ACCESS_INBOUND,
                Pipes::PIPE_TYPE_BYTE | Pipes::PIPE_WAIT,
                1,
                4 * 1024,
                4 * 1024,
                0,
                ptr::null(),
            )
        };
        if handle == Foundation::INVALID_HANDLE_VALUE {
            return Err(last_os_error("failed to create stdout pipe"));
        }
        Ok(Self(OwnedHandle(handle)))
    }

    fn connect(&self) -> UResult<()> {
        if unsafe { Pipes::ConnectNamedPipe(self.0.get(), ptr::null_mut()) } != 0 {
            return Ok(());
        }

        let error = unsafe { Foundation::GetLastError() };
        if error == Foundation::ERROR_PIPE_CONNECTED {
            return Ok(());
        }

        Err(last_os_error("failed to connect stdout pipe"))
    }

    fn copy_to_stdout(&self) -> UResult<()> {
        let mut stdout = io::stdout().lock();
        let mut buffer = [0u8; 4 * 1024];

        loop {
            let mut read = 0u32;
            if unsafe {
                FileSystem::ReadFile(
                    self.0.get(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut read,
                    ptr::null_mut(),
                )
            } == 0
            {
                let error = unsafe { Foundation::GetLastError() };
                if error == Foundation::ERROR_BROKEN_PIPE {
                    _ = stdout.flush();
                    return Ok(());
                }
                return Err(last_os_error("failed to read from stdout pipe"));
            }

            if read == 0 {
                return Ok(());
            }

            if stdout.write_all(&buffer[..read as usize]).is_err() {
                return Ok(());
            }
        }
    }
}

fn wide_null(value: impl AsRef<OsStr>) -> Vec<u16> {
    fn encode_wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }
    encode_wide(value.as_ref())
}

fn last_os_error(context: &str) -> Box<dyn UError> {
    io::Error::last_os_error().map_err_context(|| context.to_string())
}

fn reg_check_result(context: &str, result: u32) -> UResult<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result as i32).map_err_context(|| context.to_string()))
    }
}
