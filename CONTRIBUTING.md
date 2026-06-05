# Contributing

## Bug reports

If you find any bugs, we gladly accept pull requests without prior discussion.
Otherwise, you can of course always open an issue for us to look into.

## Feature requests

Please open a new issue for any feature requests you have in mind.
Since most of the behavior comes from upstream (`deps/`), new features are usually best discussed (and landed) upstream first.

## Project scope

This repository packages Unix-style command-line utilities for native Windows.
Most commands should come from one of the bundled upstream projects:

* `deps/coreutils`: GNU coreutils-compatible utilities.
* `deps/findutils`: `find` and `xargs`.
* `deps/grep`: `grep`, `egrep`, and `fgrep`.

We are not limited to GNU coreutils only. Small Windows-native commands can be
accepted when they make the Unix-like command set more useful on Windows.

The usual bar is:

* It has a close equivalent on Linux or macOS.
* That equivalent is commonly installed by default.
* People commonly use it in scripts or ordinary shell sessions.
* The Windows implementation is small enough to maintain here.
* Any Windows-specific behavior can be explained clearly.

For example, a small command such as `which` may fit this repository. So may a
Windows backend for a command that already exists upstream but is currently
Unix-only.

Commands are usually out of scope when they are not installed by default on
common Unix-like systems, are rarely used, depend on POSIX-only concepts that do
not translate to Windows, or require a large interactive terminal UI. `top`, for
example, is out of scope for now: it is mostly an interactive TUI, not a
scripting utility.

When a utility already belongs to an upstream uutils project, prefer adding or
fixing the Windows backend upstream first. Code in this repository should be for
Windows-specific glue, packaging, the multi-call wrapper, or small native
commands that do not have a better upstream home.

## Code changes

This repository is a Microsoft-maintained Windows build of upstream coreutils.
The bulk of the implementation lives in `deps/`:

* `deps/coreutils`: fork of [uutils/coreutils](https://github.com/uutils/coreutils),
  the Rust reimplementation of GNU coreutils, with Windows-patches pending upstreaming.
* `deps/findutils`: [uutils/findutils](https://github.com/uutils/findutils),
  providing `find` and `xargs`.
* `deps/grep`: fork of [uutils/grep](https://github.com/uutils/grep),
  providing `grep`, `egrep`, and `fgrep`.

When changing a utility's behavior, prefer landing the change in the relevant
upstream project first and then updating the submodule here. Windows-specific
glue, packaging, and the multi-call binary wrapper live in this repo and are
fair game for direct PRs.
