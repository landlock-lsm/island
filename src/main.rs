// SPDX-License-Identifier: Apache-2.0 OR MIT

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::generate;
use landlock::RulesetError;
use landlockconfig::{BuildRulesetError, ParseDirectoryError, ResolveError, ResolvedConfig};
use std::{
    collections::BTreeMap,
    env,
    fmt::Display,
    fs, io,
    os::unix::process::CommandExt,
    path::{self, Path, PathBuf},
    process::Command,
};
use thiserror::Error;

mod config;
use config::{is_profile_name_valid, ConfigError, IslandConfig, ResolvedProfile};

mod context;

mod workspace;

mod tests_profile;

struct Verbose(bool);

impl Verbose {
    fn print<F, T>(&self, f: F)
    where
        F: FnOnce() -> T,
        T: Display,
    {
        if self.0 {
            println!("{}", f());
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum HookShell {
    Zsh,
}

// Only list shells running on Linux.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    Zsh,
}

impl From<CompletionShell> for clap_complete::Shell {
    fn from(shell: CompletionShell) -> Self {
        match shell {
            CompletionShell::Bash => clap_complete::Shell::Bash,
            CompletionShell::Elvish => clap_complete::Shell::Elvish,
            CompletionShell::Fish => clap_complete::Shell::Fish,
            CompletionShell::Zsh => clap_complete::Shell::Zsh,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        about = "Execute a command in a sandboxed environment",
        long_about = "Run a command with Landlock security restrictions applied based on the \
            specified profile configuration. The profile directory contains TOML configuration \
            files defining the sandbox rules."
    )]
    Run {
        #[arg(
            short,
            long,
            help = "Profile name to use for sandbox configuration",
            long_help = "Name of the profile to use for sandboxing. Can be specified multiple times \
                to apply multiple profiles in order. If any -p is provided, automatic profile \
                resolution based on current working directory is disabled."
        )]
        profile: Vec<String>,

        #[arg(
            long,
            help = "Execute command without sandboxing if no profile is found"
        )]
        ignore_missing_profile: bool,

        #[arg(
            trailing_var_arg = true,
            required = true,
            num_args = 1..,
            help = "Command and arguments to execute",
            long_help = "The command to run in the sandbox followed by its arguments. Use \"--\" \
                before the command if it starts with a dash to avoid confusion with island's \
                own options."
        )]
        command: Vec<String>,
    },

    #[command(
        about = "Show the profiles that apply to the current context",
        long_about = "Check and list the profiles that would be applied if 'island run' \
            was executed in the current directory. Returns exit code 0 if profiles are found, \
            1 otherwise."
    )]
    Status,

    #[command(
        about = "Print shell integration script",
        long_about = "Output a shell script that can be sourced to integrate island with your shell. \
            Currently supports Zsh."
    )]
    Hook {
        #[arg(help = "Shell to generate integration for (currently only Zsh is supported)")]
        shell: HookShell,

        #[arg(long, help = "Output the script to remove the shell integration")]
        undo: bool,
    },

    #[command(about = "Generate shell completion scripts")]
    Completion {
        #[arg(help = "Shell to generate completion for")]
        shell: CompletionShell,
    },

    #[command(
        about = "Create a new profile",
        long_about = "Create a new profile with a default Landlock configuration."
    )]
    Create {
        #[arg(help = "Name of the new profile")]
        name: String,

        #[arg(
            short = 'b',
            long,
            default_value = ".",
            help = "Directory where the profile applies",
            long_help = "One or more directories where this profile should be automatically activated. \
                These paths will be converted to absolute paths and stored in the profile configuration."
        )]
        when_beneath: Vec<String>,
    },
    // TODO: Add profile management subcommands (list, show)
}

#[derive(Parser)]
#[command(
    name = "island",
    about = "A sandboxing tool using Landlock for secure command execution",
    long_about = "Island is a sandboxing tool that executes programs in restricted \
        environments thanks to Landlock. It applies filesystem, network, and IPC access control \
        based on profile configurations to limit what sandboxed programs can do. \
        \n \
        See https://github.com/landlock-lsm/island for more information.",
    version = concat!(env!("CARGO_PKG_VERSION"), " (commit ", env!("GIT_COMMIT"), ")")
)]
struct Cli {
    #[arg(
        short,
        long,
        global = true,
        help = "Enable verbose output showing execution steps"
    )]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Error, Debug)]
enum IslandError {
    #[error(transparent)]
    BuildRuleset(#[from] BuildRulesetError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    LandlockConfig(#[from] ParseDirectoryError),

    #[error(transparent)]
    Resolve(#[from] ResolveError),

    #[error(transparent)]
    Ruleset(#[from] RulesetError),
}

fn run(
    resolved_profiles: Vec<ResolvedProfile<'_>>,
    island_config: &IslandConfig,
    command_args: &[String],
    ignore_missing_profile: bool,
    verbose: &Verbose,
) -> Result<(), IslandError> {
    verbose.print(|| {
        format!(
            "Using {} profile(s): {}",
            resolved_profiles.len(),
            resolved_profiles
                .iter()
                .map(|p| p.name.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    });

    let mut env_vars = BTreeMap::default();

    let Some(last_profile) = resolved_profiles.last() else {
        if ignore_missing_profile {
            verbose.print(|| "No profile found, executing without sandbox".to_string());
            Err(IslandError::Io(
                Command::new(&command_args[0])
                    .args(&command_args[1..])
                    .exec(),
            ))?
        }

        // This should never happen because there is at least one resolved
        // profile returned by resolve_profiles_by_names() or
        // resolve_profiles_by_path().
        Err(IslandError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "No profile provided",
        )))?
    };

    let workspace_manager =
        last_profile.workspace_manager(island_config, verbose, |s| env::var(s))?;

    // Apply each profile's restrictions in order (broadest scope first).
    for resolved_profile in resolved_profiles {
        let (mut ruleset, rule_errors) = resolved_profile.config.build_ruleset()?;
        for rule_error in rule_errors {
            eprintln!("Warning: {}", rule_error);
        }

        // Add workspace directory access rules to ALL rulesets if the final effective
        // workspace value is true. This is necessary because Landlock uses nested
        // restrictions - if any parent ruleset doesn't allow workspace access,
        // child rulesets can't grant it either.
        ruleset = workspace_manager.update_ruleset(ruleset, verbose)?;

        // TODO: Do not rely on the kernel to enforce nested sandboxing (limited to 16 layers).
        ruleset.restrict_self()?;

        // Set environment variables defined by profile.  When using context
        // inference (e.g. resolve_profiles_by_path), the environment variables
        // are set from the broadest context to the most fitting one to favor
        // the most precise profile, potentially overriding previous ones.  This
        // is not the case when using explicit profiles (e.g.
        // resolve_profiles_by_names), where the environment variables are set
        // in the order of the provided profile names.
        for env in resolved_profile.env_vars {
            verbose.print(|| format!("Setting {}={}", env.name, env.literal));
            env_vars.insert(&env.name, &env.literal);
        }
    }

    // Add workspace environment variables to the environment that will be passed to the child process
    workspace_manager.update_environment(&mut env_vars, verbose);

    // TODO: Parse and apply --env arguments

    // Clap ensures command_args contains at least one element due to num_args = 1..
    verbose.print(|| format!("Executing: {}", command_args[0]));
    Err(IslandError::Io(
        // Inherits all file descriptors.  This may include TTY FD that could be
        // used to escape the sandbox.
        // TODO: Add a TTY proxy.
        Command::new(&command_args[0])
            .args(&command_args[1..])
            .envs(&env_vars)
            .exec(),
    ))
}

// From rustc_fs_util.
fn try_canonicalize<P>(path: P) -> io::Result<PathBuf>
where
    P: AsRef<Path>,
{
    fs::canonicalize(&path).or_else(|_| path::absolute(&path))
}

fn resolve_profiles<'a>(
    island_config: &'a IslandConfig,
    profile_names: &[String],
    verbose: &Verbose,
) -> Result<Vec<ResolvedProfile<'a>>, IslandError> {
    let load_config = |name: &str| -> Result<ResolvedConfig, ConfigError> {
        island_config
            .load_landlock_config(name)
            .map_err(|e| e.into())
    };

    if !profile_names.is_empty() {
        // Use explicit profiles, without context inference.
        verbose.print(|| format!("Using explicit profiles: {:?}", profile_names));
        Ok(island_config.resolve_profiles_by_names(profile_names, load_config)?)
    } else {
        // Use automatic profile resolution based on context.
        let canonicalized_cwd = std::env::current_dir()?.canonicalize()?;
        Ok(island_config
            .resolve_profiles_by_path(canonicalized_cwd, load_config)?
            .into_iter()
            .collect())
    }
}

fn main() -> Result<(), IslandError> {
    let cli = Cli::parse();
    let verbose = Verbose(cli.verbose);

    match cli.command {
        Commands::Run {
            profile,
            command,
            ignore_missing_profile,
        } => {
            let island_config = IslandConfig::new(|s| std::env::var(s))?;
            let resolved_profiles = resolve_profiles(&island_config, &profile, &verbose)?;

            run(
                resolved_profiles,
                &island_config,
                &command,
                ignore_missing_profile,
                &verbose,
            )
        }
        Commands::Status => {
            let island_config = IslandConfig::new(|s| std::env::var(s))?;
            let resolved_profiles = resolve_profiles(&island_config, &[], &verbose)?;

            if resolved_profiles.is_empty() {
                Err(IslandError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    "No profile found for the current directory",
                )))?
            }

            let names: Vec<&str> = resolved_profiles.iter().map(|p| p.name).collect();
            println!("{}", names.join("\n"));
            Ok(())
        }
        Commands::Hook { shell, undo } => {
            match shell {
                HookShell::Zsh => {
                    if undo {
                        println!("_island_unhook 2>/dev/null || :");
                    } else {
                        println!("{}", include_str!("../assets/shell/hook.zsh"));
                    }
                }
            }
            Ok(())
        }
        Commands::Create {
            name,
            when_beneath: paths,
        } => {
            if !is_profile_name_valid(&name) {
                return Err(IslandError::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid profile name \"{}\"", name),
                )));
            }

            let config_dir = IslandConfig::config_dir(|s| std::env::var(s))?;
            let profile_dir = config_dir.join("profiles").join(&name);
            let landlock_dir = profile_dir.join("landlock");

            if profile_dir.exists() {
                Err(IslandError::Io(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("Profile \"{}\" already exists", name),
                )))?
            }

            std::fs::create_dir_all(&landlock_dir)?;

            let mut full_paths = Vec::new();
            let profile_content = paths
                .iter()
                .map(|path| {
                    let full_path = try_canonicalize(path)?;
                    let path_value = toml::Value::String(full_path.to_string_lossy().into());
                    full_paths.push(full_path);
                    Ok(format!("[[context]]\nwhen_beneath = {}", path_value))
                })
                .collect::<Result<Vec<_>, io::Error>>()?
                .join("\n\n")
                + "\n";
            std::fs::write(profile_dir.join("profile.toml"), profile_content)?;

            std::fs::write(
                landlock_dir.join("island-default-base.toml"),
                include_str!("../assets/landlock/island-default-base.toml"),
            )?;

            println!("Created profile \"{}\" in {}", name, profile_dir.display());
            println!("It applies to:");
            for path in full_paths {
                println!("- {}", path.display());
            }
            Ok(())
        }
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(
                clap_complete::Shell::from(shell),
                &mut cmd,
                name,
                &mut io::stdout(),
            );
            Ok(())
        }
    }
}
