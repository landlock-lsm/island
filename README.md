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
- Context-aware activation: Automatically applies security profiles based on your **current working directory**.
- Full environment isolation: Manages isolated workspaces (XDG directories, `TMPDIR`) in addition to access control.
- Transparent shell integration: Automatically sandboxes commands in your shell without changing your workflow.

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

We can avoid to always prefix commands with `island run` thanks to the [shell integration](#shell-integration).

### Best practice

For proper isolation, files should be organized in dedicated directory hierarchies that match profiles (e.g., `~/work/customer-a/`, `~/work/customer-b/`, `~/personal/`) rather than mixing different contexts in the same directories.

### Profile structure

```
~/.config/island/profiles/
├── customer-a
│   ├── profile.toml
│   ├── landlock
│   │   ├── base.toml
│   │   └── home-config-ro.toml
│   ├── workspace-cache -> /home/user/.cache/island-cache-profiles/customer-a
│   ├── workspace-config -> /home/user/.config/island-config-profiles/customer-a
│   ├── workspace-data -> /home/user/.local/share/island-data-profiles/customer-a
│   ├── workspace-run -> /run/user/1000/island-run-profiles/customer-a
│   ├── workspace-state -> /home/user/.local/state/island-state-profiles/customer-a
│   └── workspace-tmp -> /tmp/island-tmp-1000-customer-a-gMiVQK
└── personal
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

Workspaces are useful to easily isolate sandboxes, but they can be disabled by inserting `workspace = false` at the top of the `profile.toml` file.

This profile automatically activates when you work in the `/home/user/work/customer-a` directory, providing customer-specific configurations and isolated application state.

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

## Shell integration

Island provides a shell integration script to transparently sandbox commands.
The goal is to seamlessly integrate sandboxing into user workflow.
This integration is a convenience feature that users control: they choose which directories are sandboxed, and they can always step out of the sandbox by changing directories or disabling the hook (with `_island_unhook`).

Most types of commands are handled (e.g., relative or absolute executables, aliases), but shell functions and `eval` commands are not.

### Setup

For Zsh, add the following to your `~/.zshrc`:

```sh
source <(island hook zsh)
```

Alternatively, you can run this command to append it automatically:

```sh
echo 'source <(island hook zsh)' >> ~/.zshrc
```

This intercepts command execution.
If the current directory matches an Island profile, the command is automatically executed via `island run`.

You can check if the launched command are sandboxed with Island by looking at the XDG paths:

```sh
env | grep XDG_
```

You can bypass the sandbox for a single command by prefixing it with `nosandbox`:

```sh
nosandbox ls /
```

### Prompt

You can display the active Island status in your prompt using the `$_ISLAND_PROFILES` array variable.

```sh
setopt PROMPT_SUBST
PROMPT='%~ %B%F{yellow}${_ISLAND_PROFILES:+▣ }%f%b%# '
```

### Undo

To remove the shell integration for the current session:

```sh
source <(island hook --undo zsh)
```

### Example with shell integration

```console
$ source <(island hook zsh)

# With shell integration enabled, commands are automatically sandboxed based on
# context, so there is no need to prefix commands with island run:
$ cd ~/work/customer-a/project-foo
$ file img.jpg

$ env | grep XDG_CONFIG_HOME
XDG_CONFIG_HOME=/home/user/.config/island-config-profiles/customer-a
```

## Installation

Currently, Island must be built from source:

```sh
git clone https://github.com/landlock-lsm/island
cd island
cargo run -- run -h
```

It can be installed in `~/.cargo/bin` with:

```sh
cargo install --path .
export PATH="$PATH:$HOME/.cargo/bin"
# Call `rehash` with Zsh.
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
- Outside interactions: Connecting to services outside the sandbox (e.g., D-Bus, Wayland) that could be used to escape the sandbox. This should be mitigated by limiting access to the related sockets or implementing proxies.
- TTY-based sandbox bypass: Since Linux 6.2, the TIOCSTI IOCTL should be restricted (see `dev.tty.legacy_tiocsti` sysctl), and we are working on a TTY proxy.
- Nested sandboxing: Currently limited by kernel's 16-level Landlock nesting. This will be addressed with synthetic Landlock ruleset chaining.
- No environment filtering: No filtering of environment variables that might contain denied paths. We should parse and update common variables (e.g., `PATH`, `XDG_DATA_DIRS`, `OLDPWD`) according to the Landlock configuration.
- Profile directory exposure: If Island's own XDG directories are accessible to sandboxed processes, configuration could be modified. We should warn about such configuration issues.

## TODO

- Command auto-completion: Shell completion support for profiles and commands.
- Configuration validation: Deny unknown config properties with helpful error messages.
- Landlock variables: Add support for `landlock_variables` extension in `[[env]]` entries.
- Default profiles: Auto-create working profiles by parsing PATH and adding common directory access when no config exists.
- Profile management: Add a `create-profile <name>` command with default templates.
- Synthetic chained ruleset: Implement custom nesting to avoid 16-level Landlock kernel limit.
- Testing: Add CI and comprehensive tests for profile inference, environment variable precedence, and workspace isolation.
- Improved error messages: Better diagnostics for profile resolution failures and Landlock errors.
- Advanced profile management: List, show, and edit profiles via CLI.
- Log denied requests (with audit).
- Improve error messages.

## Contributing

Island is part of the [Landlock project](https://landlock.io). Contributions are welcome!

## FAQ

### Troubleshooting

**Q: `island run firefox` returns an error. What should I do?**

A: First, ensure a profile exists that defines the sandbox and allows execution of the program (e.g., access to `/usr`). Second, if no profile is explicitly specified, Island infers it from the current working directory, so ensure the `when_beneath` configuration matches your location.

**Q: How do I debug Island issues?**

A: Use the `--verbose` option to see detailed logs.

**Q: Why are non-existent paths ignored with just a warning?**

A: If a path doesn't exist, it poses no security risk since it cannot be accessed. However, the warning indicates a potential configuration issue that should be addressed.

**Q: I want to run a command without automatic sandboxing. How do I do that?**

A: In a shell hooked by Island, just prefix the command with `nosandbox`.

### Configuration

**Q: Is there a way to reuse configurations across profiles?**

A: Yes, you can use symlinks to share configuration files.

**Q: Is `~/` expansion supported in paths?**

A: Not directly, but upcoming variable handling will support `${home}/`.

**Q: Can a context be based on the program instead of the working directory?**

A: While feasible, Island focuses on data-centric security policies (based on location/project) rather than application-centric ones.

**Q: What happens if multiple profiles' contexts match the current working directory?**

A: Island applies all matching profiles in order of specificity (broadest path first). The restrictions are combined, meaning access is only granted if *all* matching profiles allow it.

### Concepts & Security

**Q: What are XDG directories?**

A: These are standard directories defined by the XDG Base Directory specification (e.g., `.config`, `.local/share`) used by modern applications to store configuration and data.

**Q: What is the cache directory used for?**

A: Island itself doesn't use a cache, but sandboxed applications may use `$XDG_CACHE_HOME` for temporary storage.

**Q: How do I launch a graphical application or use D-Bus services?**

A: The relevant sockets must be exposed in `$XDG_RUNTIME_DIR` (e.g., via symlinks) and allowed by the profile. Note that exposing these services extend (or bypass) the sandbox restrictions.

**Q: What if a profile allows modification of Island's configuration?**

A: This would be a security issue to fix in the profile configuration.
