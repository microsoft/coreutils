<h1 align="center">Coreutils for Windows</h1>

<p align="center">UNIX-style core utilities for Windows. The same commands and pipelines you use on Linux, macOS, and WSL - natively.</p>

<h3 align="center">
  <a href="#install">Install</a>
  <span> · </span>
  <a href="#shell-conflicts">Shell conflicts</a>
  <span> · </span>
  <a href="#windows-caveats">Windows caveats</a>
  <span> · </span>
  <a href="#contributing">Contributing</a>
</h3>

---

A Microsoft-maintained build of [uutils/coreutils](https://github.com/uutils/coreutils),
[findutils](https://github.com/uutils/findutils), and [grep](https://github.com/uutils/grep) packaged as a
single multi-call binary for Windows. The goal is to make moving between Linux, macOS, WSL,
containers, and Windows frictionless: the same commands, flags, and pipelines work the same
way, so existing scripts carry over without translation.

Each command supports the standard `--help` flag for full syntax and options.

**This project is in preview.**

<br/>

## Install

Install Coreutils for Windows with WinGet:

```powershell
winget install Microsoft.Coreutils
```

Or grab the latest build from our [Release Page](https://github.com/microsoft/coreutils/releases/latest).

<br/>

## Shell conflicts

> [!NOTE]
> Any command not mentioned is included in this suite. The following only lists conflicts.

> [!WARNING]
> PowerShell 7.4 or newer is required. Older PowerShell versions aren't supported.

Several commands share names with built-ins in CMD and PowerShell. Whether the Coreutils
version runs depends on the shell, the PATH order, and (for PowerShell) the alias table.

Legend: ✅ ships and works · ⚠️ ships but conflicts with a built-in · 🛑 not shipped

| Command    | CMD  | PowerShell 7.4+ | Notes |
| ---------- | :--: | :-------------: | ----- |
| `cat`      |  ✅  |       ⚠️        | |
| `cp`       |  ✅  |       ⚠️        | |
| `date`     |  ⚠️  |       ⚠️        | |
| `dir`      |  🛑  |       🛑        | Conflicts with the built-in DOS command |
| `echo`     |  ⚠️  |       ⚠️        | |
| `expand`   |  🛑  |       🛑        | Conflicts with the built-in DOS command |
| `find`     |  ✅  |       ✅        | Integrated port of the original DOS command |
| `hostname` |  ✅  |       ✅        | Superset of the Windows built-in |
| `kill`     |  🛑  |       🛑        | Unavailable due to lack of signals on Windows; Implementing a form of SIGTERM/SIGKILL may be possible in the future however |
| `ls`       |  ✅  |       ⚠️        | |
| `mkdir`    |  ⚠️  |       ⚠️        | |
| `more`     |  🛑  |       🛑        | Conflicts with the built-in DOS command (consider `edit` as an alternative) |
| `mv`       |  ✅  |       ⚠️        | |
| `paste`    |  🛑  |       🛑        | Conflicts with the built-in DOS command |
| `pwd`      |  ✅  |       ⚠️        | |
| `rm`       |  ✅  |       ⚠️        | |
| `rmdir`    |  ⚠️  |       ⚠️        | |
| `sleep`    |  ✅  |       ⚠️        | |
| `sort`     |  ✅  |       ✅        | Integrated port of the original DOS command |
| `tee`      |  ✅  |       ⚠️        | |
| `timeout`  |  🛑  |       🛑        | Relies on `kill`'s functionality |
| `uptime`   |  ✅  |       ⚠️        | |
| `whoami`   |  🛑  |       🛑        | Conflicts with the built-in Windows command |

<br/>

## Windows caveats

| Difference            | Detail |
| --------------------- | ------ |
| **CRLF line endings** | Windows text files often use CRLF (`\r\n`). Most utilities handle this transparently, but pattern matching with `$` and exact byte counts can be affected. |
| **No `/dev/null`**    | Use `NUL` instead, for example `find . -name "*.log" > NUL` |
| **No POSIX signals**  | Signals such as `SIGHUP`, `SIGPIPE`, and `SIGUSR` aren't available. `Ctrl+C` (`SIGINT`) works as expected. |
| **Path separators**   | Both `/` and `\` are accepted. Some utilities produce `\`-separated output, which can affect downstream piping. |
| **File permissions**  | Windows uses ACLs, not POSIX permission bits. Permission-based predicates (for example `find -perm`) may behave differently or be unavailable. |
| **Symbolic links**    | Reading existing symbolic links works without elevation. Creating new symbolic links requires Developer Mode ([**Settings > System > Advanced**](https://learn.microsoft.com/windows/advanced-settings)) or an elevated terminal. |

### PowerShell Command Parsing

The installer integrates itself with interactive PowerShell sessions via `PSReadLine`.
It ensures that quoted expression behave somewhat like they do under UNIX shells or CMD:
`echo *.txt` will then print a number of file names, while `echo '*.txt'` will print "*.txt" literally.

There are two shortcomings, however:
* PowerShell's escape character is still <code>\`</code>, not <code>\\</code><br>
  While you may write `find . \( -foo -bar \)` with Bash, you still need to write ``find . `( -foo -bar `)`` in PowerShell.
* `Get-Command ls`, `Get-Help ls`, etc., will still show `ls`, etc., as builtin commands<br>
  Due to limitations around `PSNativeCommandPreserveBytePipe` we cannot integrate ourselves in a more robust way with PowerShell.

### Intentionally dropped

Commands that exist upstream but aren't shipped here because they rely on POSIX-only concepts, would break existing Windows scripts, or simply aren't useful on Windows.

* `dd`: Perhaps useful in the future.
* `dircolors`, `shred`, `sync`, `uname`: Not particularly useful on Windows.
* `chcon`, `chgrp`, `chmod`, `chown`, `chroot`, `groups`, `hostid`, `id`, `install`,
  `logname`, `mkfifo`, `mknod`, `nice`, `nohup`, `pathchk`, `pinky`, `runcon`, `stdbuf`,
  `stty`, `tty`, `users`, `who`: POSIX-only concepts unavailable on Windows.

<br/>

## Contributing

Bug reports and pull requests are welcome. See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for details on the repo layout and how changes flow between this repo and the upstream uutils projects.
