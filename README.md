# Island 🏝️

A sandboxing tool using Landlock for secure command execution.

> ⚠️ **Work in progress:** Island is currently under active development and not yet ready for production use.

## Overview

[Landlock](https://landlock.io) is a Linux Security Module (LSM) that empowers unprivileged processes to securely restrict their own access rights (e.g., filesystem, network).
While Landlock provides powerful kernel primitives, using it typically requires modifying application code.

Island makes Landlock practical for everyday workflows by acting as a high-level wrapper and policy manager.
Developed alongside the kernel feature and its Rust libraries, it bridges the gap between raw security mechanisms and user activity through:

- Zero-code integration: Runs existing binaries without modification.
- Declarative policies: Uses TOML profiles instead of code-based rules.
- Context-aware activation: Automatically applies security profiles based on your current working directory.
- Full environment isolation: Manages isolated workspaces (XDG directories, `TMPDIR`) in addition to access control.

By tying security policies to locations rather than just applications, Island enables natural, invisible sandboxing where `cd project && island run -- command` just works.

## How it works

Profiles are stored in `~/.config/island/profiles/<name>/` and contain:
- `profile.toml`: Defines the activation context, environment variables, and workspace isolation settings.
- `landlock/`: Contains security policy files (e.g., filesystem access rules) using the [Landlock Config](https://github.com/landlock-lsm/landlockconfig) format.
- Workspace symlinks: Automatic isolation directories.

The `profile.toml` file defines two main configuration areas:

- Context matching `[[context]]`: Determines when profiles activate automatically. Using the `when_beneath` key, you can bind security policies to specific directory paths.
- Environment configuration `[[env]]`: Sets environment variables that are injected into the sandboxed process.

Multiple profiles can match a single execution context, creating layered security.
Profiles are applied in order of specificity (broadest first), with each layer adding further restrictions.

Workspace isolation ensures clean environments for each activity:
- Persistent storage: Each profile maintains separate XDG directories (config, data, state, cache) that persist between runs. Additionally, each context path (`when_beneath`) is automatically made accessible to the sandboxed process with this profile.
- Ephemeral storage: A unique, secure `TMPDIR` and runtime directory are generated for each execution to prevent data leakage, based on system defaults (e.g., `/tmp`). These directories' lifetime is managed by the system.
- Automatic integration: Applications automatically use these isolated paths via injected environment variables.

Environment integration provides context awareness:
- The `ISLAND_CONTEXT_BENEATH_[0-9]+` variables expose all `when_beneath` paths related to the currently used profile.
- Profile-specific environment variables (from `profile.toml`) are injected.
- Automatic workspace directory configuration sets standard XDG variables (e.g., `XDG_CONFIG_HOME`) to isolated and automatically allowed paths.

Execution process:
1. Profile selection: Explicit (`-p name`) or automatic (directory matching).
2. Workspace creation: Set up isolated XDG directories with symlinks.
3. Environment configuration: Apply profile variables and context paths.
4. Security setup: Load Landlock policies and create nested restrictions.
5. Command execution: Run target program in the secured environment.

### Example of activity-based isolation

Island profiles map sandbox environments to user activities, making it natural to organize work by context.
In these examples, two profiles are defined: customer-a (with the `when_beneath = "/home/user/work/customer-a"` context) and personal (without context).

```sh
# Run with automatic profile resolution (based on current directory) with verbose mode.
cd ~/work/customer-a/project-foo
island -v run vim README.md

# Run with explicit profile for personal vs work activities with different configurations.
island run -p customer-a firefox
island run -p personal firefox
```

### Best practice

For proper isolation, files should be organized in dedicated directory hierarchies that match profiles (e.g., `~/work/customer-a/`, `~/work/customer-b/`, `~/personal/`) rather than mixing different contexts in the same directories.

### Profile structure

```
~/.config/island/profiles/
├── project-a
│   ├── profile.toml
│   ├── landlock
│   │   ├── base.toml
│   │   └── home-config-ro.toml
│   ├── workspace-cache -> /home/user/.cache/island-cache-profiles/project-a
│   ├── workspace-config -> /home/user/.config/island-config-profiles/project-a
│   ├── workspace-data -> /home/user/.local/share/island-data-profiles/project-a
│   ├── workspace-run -> /run/user/1000/island-run-profiles/project-a
│   ├── workspace-state -> /home/user/.local/state/island-state-profiles/project-a
│   └── workspace-tmp -> /tmp/island-tmp-1000-project-a-gMiVQK
└── work-project
    ├── profile.toml
    ├── landlock
    │   └── strict.toml
    └── workspace-* symlinks...
```

The workspace symlinks are automatically created and managed by Island. They provide:

- Isolation: Each profile's applications see only their own configuration and data.
- Easy introspection: You can easily see which directories a profile uses by examining its symlinks.
- XDG compliance: Programs supporting the XDG specification automatically benefit from this isolation.
- Development workflow: Different profiles can have completely different tool configurations (e.g., different IDE settings, Git configs, SSH keys).

Example `profile.toml`:

```toml
# Apply this profile when working in customer-a project directory.
[[context]]
when_beneath = "/home/user/work/customer-a"

# Set environment variables for this activity.
[[env]]
name = "EDITOR"
literal = "/usr/bin/vim"

[[env]]
name = "CUSTOMER_NAME"
literal = "The A-Team"
```

This profile automatically activates when you work in the `/home/user/work/customer-a/` directory, providing customer-specific configurations and isolated application state.

Example Landlock configuration (`landlock/base.toml`):
```toml
abi = 6

# Deny all supported access by default.
[[ruleset]]
handled_access_fs = ["abi.all"]
handled_access_net = ["abi.all"]
scoped = ["abi.all"]

# Allow read/execute access to system directories.
[[path_beneath]]
allowed_access = ["abi.read_execute"]
parent = ["/bin", "/lib", "/usr", "/etc"]

# Allow read/write access to a shared directory.
[[path_beneath]]
allowed_access = ["abi.read_write"]
parent = ["/home/user/shared"]
```

See the [Landlock Config documentation](https://github.com/landlock-lsm/landlockconfig) for complete access control configuration options.

## Features

### Workflow

- Directory-driven security: Profiles activate automatically based on your current location.
- Zero application changes: Works with any existing program that supports XDG directories.
- Easy profile management: Self-contained profile directories that can be shared and version controlled.
- Context awareness: Environment variables expose matched paths for scripts and applications.
- Flexible usage: Support both explicit profile selection and automatic directory-based activation.

### Security

- Zero-privilege operation: No root access or special capabilities required.
- Layered protection: Multiple profiles compose cleanly with deterministic ordering. Landlock restrictions accumulate across multiple profiles (broadest to most specific). Since Landlock permissions are intersected, a child profile can only *reduce* access granted by parent profiles, not expand it.
- Complete environment isolation: XDG-compliant workspace separation for configurations, data, and state. Each profile gets separate temporary and XDG directories.
- Workspace validation: Prevents symlink attacks through path canonicalization and file ownership checks.

## Installation

Currently, Island must be built from source:

```sh
git clone https://github.com/landlock-lsm/island
cd island
cargo run -- run -h
```

It can be installed for the current user with:
```sh
cargo install --path .
```

## Requirements

- Linux with Landlock support (kernel 5.13+).
- Rust 1.82 or later for building from source.

## Security model and current limitations

Island is designed to protect against:
- Malicious code accessing unintended files or directories.
- Configuration and data contamination between different security contexts.
- Sibling users and sandboxes (e.g., workspace interference).

Island is **not** designed to protect against (non-exhaustive list):
- Kernel or privileged service vulnerabilities.
- Resource exhaustion attacks (CPU, memory, disk space).

Current limitations:
- TTY-based sandbox escapes: Inherits the current TTY file descriptor, which can be used to escape the sandbox. We are working on a TTY proxy.
- Outside interactions: Connecting to services outside the sandbox (e.g., D-Bus, Wayland) that could be used to escape the sandbox. This should be mitigated by limiting access to the related sockets or implementing proxies.
- Nested sandboxing: Currently limited by kernel's 16-level Landlock nesting. This will be addressed with synthetic Landlock ruleset chaining.
- No environment filtering: No filtering of environment variables that might contain denied paths. We should parse and update common variables (e.g., `PATH`, `XDG_DATA_DIRS`, `OLDPWD`) according to the Landlock configuration.
- Profile directory exposure: If Island's own XDG directories are accessible to sandboxed processes, configuration could be modified. We should warn about such configuration issue.

## TODO

- Command auto-completion: Shell completion support for profiles and commands.
- Shell integration: Integrate into shell pre-exec hook for transparent operation.
- Configuration validation: Deny unknown config properties with helpful error messages.
- Landlock variables: Add support for `landlock_variables` extension in `[[env]]` entries.
- Default profiles: Auto-create working profiles by parsing PATH and adding common directory access when no config exists.
- Profile management: Add a `create-profile <name>` command with default templates.
- Synthetic chained ruleset: Implement custom nesting to avoid 16-level Landlock kernel limit.
- Testing: Add CI and comprehensive tests for profile inference, environment variable precedence, and workspace isolation.
- Improved error messages: Better diagnostics for profile resolution failures and Landlock errors.
- Advanced profile management: List, show, and edit profiles via CLI.

## Contributing

Island is part of the [Landlock project](https://landlock.io). Contributions are welcome!
