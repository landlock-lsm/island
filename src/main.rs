// SPDX-License-Identifier: Apache-2.0 OR MIT

use clap::{Parser, Subcommand};
use landlock::RulesetError;
use landlockconfig::{BuildRulesetError, ParseDirectoryError, ResolveError, ResolvedConfig};
use std::{fmt::Display, os::unix::process::CommandExt, process::Command};
use thiserror::Error;

mod config;
use config::{ConfigError, IslandConfig, ResolvedProfile};

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
    command_args: &[String],
    verbose: &Verbose,
) -> Result<(), IslandError> {
    verbose.print(|| {
        format!(
            "Using {} profile(s): {}",
            resolved_profiles.len(),
            resolved_profiles
                .iter()
                .map(|p| p.entry.name.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    });

    // Apply each profile's restrictions in order (broadest scope first).
    for resolved_profile in resolved_profiles {
        let (ruleset, rule_errors) = resolved_profile.config.build_ruleset()?;
        for rule_error in rule_errors {
            eprintln!("Warning: {}", rule_error);
        }
        // TODO: Do not rely on the kernel to enforce nested sandboxing (limited to 16 layers).
        ruleset.restrict_self()?;
    }

    // TODO: Apply environment variable modifications from profile
    // TODO: Parse and apply --env arguments

    // Clap ensures command_args contains at least one element due to num_args = 1..
    verbose.print(|| format!("Executing: {}", command_args[0]));
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
    let verbose = Verbose(cli.verbose);

    match cli.command {
        Commands::Run { profile, command } => {
            let island_config = IslandConfig::load()?;
            let load_config = |name: &str| -> Result<ResolvedConfig, ConfigError> {
                island_config
                    .load_landlock_config(name)
                    .map_err(|e| e.into())
            };

            let resolved_profiles = if !profile.is_empty() {
                // Use explicit profiles - no CWD inference.
                verbose.print(|| format!("Using explicit profiles: {:?}", profile));
                island_config.resolve_profiles_by_names(&profile, load_config)?
            } else {
                // Use automatic profile resolution based on CWD.
                let canonicalized_cwd = std::env::current_dir()?.canonicalize()?;
                island_config.resolve_profiles_by_path(canonicalized_cwd, load_config)?
            };

            run(resolved_profiles, &command, &verbose)
        }
    }
}
