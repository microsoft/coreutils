# Windows-Native Command Notes

Some commands may need a small Windows-native implementation when the upstream
submodules do not provide a usable Windows backend.

Before adding a new command, please open an issue first so maintainers can agree
on the command scope and option support.

Prefer small, script-friendly commands that are commonly available by default on
Linux or macOS and can be implemented on Windows with clear, documented
approximations. Interactive, non-default, or broad system-management tools should
be discussed first because they can add significant maintenance cost.

For GNU-compatible options:

- Define options to match the GNU interface and upstream behavior where
  practical.
- Implement options that can reasonably work on Windows, even if approximated.
- Return an error for options that cannot work on Windows but are important to
  the command's behavior.
- Document Windows-specific approximations or unsupported behavior near the
  option definitions, or in user-facing docs when needed.
