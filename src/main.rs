// SPDX-License-Identifier: Apache-2.0 OR MIT

use clap::{Parser, Subcommand};
use landlock::RulesetError;
use landlockconfig::{
    BuildRulesetError, Config, ConfigFormat, ParseDirectoryError, ResolveError, ResolvedConfig,
};
use std::{
    io::{Error, ErrorKind},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};
use thiserror::Error;

#[derive(Subcommand)]
enum Commands {
    #[command(
        about = "Execute a command in a sandboxed environment",
        long_about = "Run a command with Landlock security restrictions applied based on the \
            specified profile configuration. The profile directory must contain a \"landlock\" \
            subdirectory with TOML configuration files defining the sandbox rules."
    )]
    Run {
        #[arg(
            short,
            long,
            help = "Profile directory containing the sandbox configuration",
            long_help = "Path to the profile directory that contains a \"landlock\" subdirectory \
                with TOML configuration files. These files define the filesystem and network \
                access rules for the sandbox."
        )]
        profile: PathBuf,

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
    // TODO: Add profile management subcommands (list, show, create)
}

#[derive(Parser)]
#[command(
    name = "island",
    about = "A sandboxing tool using Landlock for secure command execution",
    long_about = "Island is a command-line tool that executes programs in a restricted \
        environment using Linux's Landlock security module. It applies filesystem and network \
        access controls based on profile configurations to limit what sandboxed programs can do.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Error, Debug)]
enum IslandError {
    #[error(transparent)]
    BuildRuleset(#[from] BuildRulesetError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Landlock configuration directory not found: {path}")]
    LandlockConfigNotFound { path: String },

    #[error(transparent)]
    LandlockConfig(#[from] ParseDirectoryError),

    #[error(transparent)]
    Resolve(#[from] ResolveError),

    #[error(transparent)]
    Ruleset(#[from] RulesetError),
}

fn load_profile(profile_dir: &Path) -> Result<ResolvedConfig, IslandError> {
    let landlock_dir = profile_dir.join("landlock");

    let landlock_metadata =
        landlock_dir
            .metadata()
            .map_err(|_| IslandError::LandlockConfigNotFound {
                path: landlock_dir.display().to_string(),
            })?;

    if !landlock_metadata.is_dir() {
        return Err(Error::new(
            ErrorKind::NotADirectory,
            format!("Path is not a directory: {}", landlock_dir.display()),
        )
        .into());
    }

    Ok(Config::parse_directory(&landlock_dir, ConfigFormat::Toml)?.resolve()?)
}

fn run(profile_dir: &Path, command_args: &[String]) -> Result<(), IslandError> {
    let landlock_config = load_profile(profile_dir)?;

    let (ruleset, rule_errors) = landlock_config.build_ruleset()?;

    for rule_error in rule_errors {
        eprintln!("Warning: {}", rule_error);
    }

    ruleset.restrict_self()?;

    // TODO: Apply environment variable modifications from profile
    // TODO: Parse and apply --env arguments

    // Clap ensures command_args contains at least one element due to num_args = 1..
    Err(IslandError::Io(
        // Inherits all file descriptors.  This may include TTY FD that could be
        // used to escape the sandbox.
        // TODO: Add a TTY proxy.
        Command::new(&command_args[0])
            .args(&command_args[1..])
            .exec(),
    ))
}

fn main() -> Result<(), IslandError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { profile, command } => run(&profile, &command),
    }
}
