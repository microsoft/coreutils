# Contributing

## Bug reports

If you find any bugs, we gladly accept pull requests without prior discussion.
Otherwise, you can of course always open an issue for us to look into.

## Feature requests

Please open a new issue for any feature requests you have in mind.  Since most of the behavior comes from upstream (`deps/`), new features are usually best discussed (and landed) upstream first.

## Adopting Commands

We'll adopt a command by command basis. Some command make sense on Windows, others do not.  Please create a feature request for an additional commands.  As stated above, most behavior is driven from upstream.

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
