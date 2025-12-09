// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Workspace environment management for Island profiles.
//!
//! Provides isolated workspace directories for each profile, managing XDG
//! directories and `TMPDIR`.  Each profile gets its own separate workspace to
//! prevent interference between sandboxed environments.

use crate::{
    config::{IslandConfig, Profile, ResolvedProfile},
    lock::{ExclusiveLock, ProfileLock, SharedLock, SharedLockError},
    IslandError, Verbose,
};
use landlock::{path_beneath_rules, Access, AccessFs, RulesetCreatedAttr, ABI};
use std::{
    collections::{BTreeMap, HashMap},
    env::VarError,
    fs, io,
    os::unix::fs::{symlink, MetadataExt},
    path::PathBuf,
};

/// XDG directories
///
/// These directories should all be created in directories own by the current
/// user.
// TODO: We should probably handle XDG_DOWNLOAD_DIR per profile as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Xdg {
    /// XDG configuration directory (`XDG_CONFIG_HOME`)
    ConfigHome,
    /// XDG data directory (`XDG_DATA_HOME`)
    DataHome,
    /// XDG state directory (`XDG_STATE_HOME`)
    StateHome,
    /// XDG cache directory (`XDG_CACHE_HOME`)
    CacheHome,
    /// XDG runtime directory (`XDG_RUNTIME_DIR`)
    RuntimeDir,
}

impl Xdg {
    /// Returns the environment variable name for this XDG directory.
    const fn env_var(&self) -> &'static str {
        match self {
            Xdg::ConfigHome => "XDG_CONFIG_HOME",
            Xdg::DataHome => "XDG_DATA_HOME",
            Xdg::StateHome => "XDG_STATE_HOME",
            Xdg::CacheHome => "XDG_CACHE_HOME",
            Xdg::RuntimeDir => "XDG_RUNTIME_DIR",
        }
    }

    /// Returns the symlink name used in profile directories.
    const fn symlink_name(&self) -> &'static str {
        match self {
            Xdg::ConfigHome => "workspace-config",
            Xdg::DataHome => "workspace-data",
            Xdg::StateHome => "workspace-state",
            Xdg::CacheHome => "workspace-cache",
            Xdg::RuntimeDir => "workspace-run",
        }
    }

    /// Returns the fallback path relative to `$HOME`, or `None` if no fallback exists.
    const fn fallback_path(&self) -> Option<&'static str> {
        match self {
            Xdg::ConfigHome => Some(".config"),
            Xdg::DataHome => Some(".local/share"),
            Xdg::StateHome => Some(".local/state"),
            Xdg::CacheHome => Some(".cache"),
            Xdg::RuntimeDir => None,
        }
    }

    /// Returns the subdirectory name used for Island profile isolation.
    const fn subdir(&self) -> &'static str {
        match self {
            Xdg::ConfigHome => "island-config-profiles",
            Xdg::DataHome => "island-data-profiles",
            Xdg::StateHome => "island-state-profiles",
            Xdg::CacheHome => "island-cache-profiles",
            Xdg::RuntimeDir => "island-run-profiles",
        }
    }
}

/// Workspace directory types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymlinkWorkspace {
    /// XDG Base Directory specification directory
    Xdg(Xdg),
    /// Temporary directory
    TmpDir,
}

impl SymlinkWorkspace {
    /// Returns the environment variable name for this workspace type.
    const fn env_var(&self) -> &'static str {
        match self {
            SymlinkWorkspace::Xdg(xdg) => xdg.env_var(),
            SymlinkWorkspace::TmpDir => "TMPDIR",
        }
    }

    /// Returns the symlink name used in profile directories.
    const fn symlink_name(&self) -> &'static str {
        match self {
            SymlinkWorkspace::Xdg(xdg) => xdg.symlink_name(),
            SymlinkWorkspace::TmpDir => "workspace-tmp",
        }
    }

    /// Resolves a workspace path safely, validating ownership for temporary environments.
    ///
    /// Returns the canonicalized target path after performing security validations.
    /// For temporary directories, validates that symlinks and their first-level targets
    /// are owned by the current user. For XDG directories, performs basic path resolution.
    /// Once ownership is validated, trusts the user's symlink configurations.
    ///
    /// This prevents multiple attack vectors:
    ///
    /// **Post-reboot attacks**: Other users create malicious symlinks after system cleanup
    /// when temp directories are removed but profile symlinks remain. This is detected by
    /// validating that both symlink and first-level target have matching ownership.
    ///
    /// **Confused deputy attacks**: Other users or processes make symlinks point to
    /// legitimate directories owned by the current user (like ~/.ssh or ~/Documents) causing
    /// temp files to be written in unexpected locations. Protection is provided because:
    /// 1. Island creates symlinks pointing to directories Island also created.
    /// 2. Both symlink and target are owned by the current user.
    /// 3. If someone changes the symlink to point elsewhere, they must also own
    ///    that target location for validation to pass.
    /// 4. If they own the target but aren't the current user, validation fails.
    /// 5. If they are the current user, it's their own directory so no privilege escalation.
    ///
    /// **Shared /tmp threat model**: In traditional shared /tmp directories, different
    /// processes or sandboxes share the same temp space, creating information leakage
    /// risks both within and across users. Each Island profile gets its own isolated
    /// temporary directory (e.g., `/tmp/island-tmp-1000-profile1-abc123/`) to prevent:
    /// - Cross-user interference when multiple users run Island.
    /// - Cross-profile data leakage between different Landlock sandboxes of same user.
    /// - One sandbox reading temporary files from another sandbox.
    /// - Profile isolation bypass through shared temporary storage.
    ///
    /// The UID in the directory name provides namespace separation between users,
    /// while the profile name provides separation between different sandboxes
    /// of the same user. File ownership validation handles cross-user security,
    /// while directory separation handles intra-user isolation.
    ///
    /// The user's own symlink configurations are trusted after validating the security boundary.
    fn resolve_directory(&self, path: &PathBuf) -> io::Result<PathBuf> {
        // XXX: Should we really canonicalize?  This means that the directories
        // cannot be changed an runtime.
        let canonical_path = match self {
            SymlinkWorkspace::TmpDir => {
                if path.is_symlink() {
                    // Get symlink metadata (not following the link).
                    let symlink_metadata = fs::symlink_metadata(path)?;
                    let symlink_uid = symlink_metadata.uid();

                    // Read the symlink target (first level only).
                    let target_path = std::fs::read_link(path)?;

                    // Convert relative paths to absolute based on symlink's parent.
                    let absolute_target = if target_path.is_absolute() {
                        target_path
                    } else {
                        path.parent()
                            .ok_or_else(|| {
                                std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,
                                    "Symlink has no parent directory",
                                )
                            })?
                            .join(&target_path)
                    };

                    // Get first-level target metadata (not following further symlinks).
                    let target_metadata = fs::symlink_metadata(&absolute_target)?;
                    let target_uid = target_metadata.uid();

                    // Validate symlink and target ownership match.
                    if symlink_uid != target_uid {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!(
                                "Security: Symlink {} (UID {}) points to target owned by different UID {}: {}",
                                path.display(),
                                symlink_uid,
                                target_uid,
                                absolute_target.display()
                            ),
                        ));
                    }

                    // Return canonicalized target (trust user's symlink chains
                    // after ownership validation).
                    absolute_target.canonicalize()?
                } else {
                    // For non-symlinks, just return the canonical path.
                    path.canonicalize()?
                }
            }
            SymlinkWorkspace::Xdg(_) => {
                // For XDG directories, just return the canonical path.
                path.canonicalize()?
            }
        };

        // Ensure the final target is a directory for workspace consistency.
        let metadata = fs::metadata(&canonical_path)?;
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Workspace path {} is not a directory",
                    canonical_path.display()
                ),
            ));
        }

        Ok(canonical_path)
    }

    /// Returns the default path for this workspace environment.
    pub fn create_directory<E>(
        &self,
        profile_name: &str,
        original_env: &HashMap<String, Option<String>>,
        read_env: E,
    ) -> io::Result<PathBuf>
    where
        E: Fn(&str) -> Result<String, VarError>,
    {
        match self {
            SymlinkWorkspace::TmpDir => {
                // Get effective user ID for name collision avoidance Use
                // effective UID since that's what determines ownership of newly
                // created files/directories and matches what tempfile will
                // actually use for directory ownership.
                let current_uid = unsafe { libc::geteuid() };

                let temp_dir = tempfile::Builder::new()
                    .prefix(&format!("island-tmp-{}-{}-", current_uid, profile_name))
                    .tempdir_in(read_temp_dir(read_env))
                    .map_err(|e| {
                        std::io::Error::other(format!(
                            "Failed to create secure temporary directory: {}",
                            e
                        ))
                    })?;
                Ok(temp_dir.keep())
            }
            SymlinkWorkspace::Xdg(xdg) => {
                // Resolve XDG directory path with fallbacks.
                let base_path = if let Some(env_value) =
                    original_env.get(xdg.env_var()).and_then(|opt| opt.as_ref())
                {
                    PathBuf::from(env_value)
                } else if let Some(fallback) = xdg.fallback_path() {
                    let home = std::env::var("HOME").map_err(|_| {
                        io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!(
                                "Neither {} nor HOME environment variables are set",
                                xdg.env_var()
                            ),
                        )
                    })?;
                    PathBuf::from(home).join(fallback)
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("{} environment variable is not set", xdg.env_var()),
                    ));
                };
                let dir = base_path.join(xdg.subdir()).join(profile_name);
                fs::create_dir_all(&dir)?;
                Ok(dir)
            }
        }
    }
}

// Copied from std::env::temp_dir() and adapted to not use env::var but read_env.
fn read_temp_dir<E>(read_env: E) -> PathBuf
where
    E: Fn(&str) -> Result<String, VarError>,
{
    read_env("TMPDIR").map(PathBuf::from).unwrap_or_else(|_| {
        #[cfg(target_os = "android")]
        {
            PathBuf::from("/data/local/tmp")
        }
        #[cfg(not(target_os = "android"))]
        {
            PathBuf::from("/tmp")
        }
    })
}

/// All supported workspace environment variables.
const SYMLINK_WORKSPACES: &[SymlinkWorkspace] = &[
    SymlinkWorkspace::Xdg(Xdg::ConfigHome),
    SymlinkWorkspace::Xdg(Xdg::DataHome),
    SymlinkWorkspace::Xdg(Xdg::StateHome),
    SymlinkWorkspace::Xdg(Xdg::CacheHome),
    SymlinkWorkspace::Xdg(Xdg::RuntimeDir),
    SymlinkWorkspace::TmpDir,
];

enum AbsoluteWorkspace {
    ContextBeneath { index: usize, path: PathBuf },
}

impl AbsoluteWorkspace {
    fn new(profile: &Profile) -> Vec<Self> {
        profile
            .contexts
            .iter()
            .flat_map(|c| c.when_beneath.as_ref())
            .enumerate()
            .map(|(index, p)| Self::ContextBeneath {
                index,
                path: p.clone(),
            })
            .collect()
    }

    fn env_var(&self) -> String {
        match self {
            AbsoluteWorkspace::ContextBeneath { index, .. } => {
                format!("ISLAND_CONTEXT_BENEATH_{}", index)
            }
        }
    }

    const fn target_path(&self) -> &PathBuf {
        match self {
            AbsoluteWorkspace::ContextBeneath { path, .. } => path,
        }
    }
}

/// Manages workspace setup and configuration for a profile.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct WorkspaceManager {
    pub(crate) env_vars: BTreeMap<String, String>,
}

impl WorkspaceManager {
    pub fn new<E>(
        resolved_profile: &ResolvedProfile,
        island_config: &IslandConfig,
        verbose: &Verbose,
        read_env: E,
    ) -> Result<Self, IslandError>
    where
        E: Fn(&str) -> Result<String, VarError>,
    {
        if !resolved_profile.workspace {
            return Ok(Default::default());
        }

        // Collect original workspace environment variables before any modifications.
        let symlink_original_env: HashMap<String, Option<String>> = SYMLINK_WORKSPACES
            .iter()
            .map(|env| {
                let env_var = env.env_var();
                (env_var.to_string(), read_env(env_var).ok())
            })
            .collect();

        // Set up workspace directories for all workspace environment variables.
        // First pass: check if changes are needed.
        match Self::resolve_workspaces::<SharedLock<IslandError>, E>(
            island_config,
            resolved_profile.name,
            resolved_profile,
            &symlink_original_env,
            &read_env,
            verbose,
        ) {
            Ok(env_vars) => Ok(Self { env_vars }),
            Err(SharedLockError::Inner(e)) => Err(e),
            Err(SharedLockError::NeedsUpdate) => {
                // Second pass: potentially perform changes.
                Ok(Self {
                    env_vars: Self::resolve_workspaces::<ExclusiveLock<IslandError>, E>(
                        island_config,
                        resolved_profile.name,
                        resolved_profile,
                        &symlink_original_env,
                        &read_env,
                        verbose,
                    )?,
                })
            }
        }
    }

    fn resolve_workspaces<L, E>(
        island_config: &IslandConfig,
        profile_name: &str,
        resolved_profile: &ResolvedProfile,
        symlink_original_env: &HashMap<String, Option<String>>,
        read_env: &E,
        verbose: &Verbose,
    ) -> Result<BTreeMap<String, String>, L::Error>
    where
        L: ProfileLock,
        L::Error: From<IslandError> + From<io::Error>,
        E: Fn(&str) -> Result<String, VarError>,
    {
        let mut env_vars: BTreeMap<String, String> = BTreeMap::new();

        let profile_guard = island_config.guard_profile::<L>(profile_name)?;

        for workspace in SYMLINK_WORKSPACES {
            let env_var = workspace.env_var();
            let symlink_path = profile_guard.path().join(workspace.symlink_name());

            // Try to use existing path, removing if error (e.g. broken symlink).
            let target_path = if symlink_path.exists() {
                // Validate ownership and resolve path securely for this
                // workspace environment type.
                match workspace.resolve_directory(&symlink_path) {
                    Ok(resolved_path) => {
                        verbose.print(|| {
                            if symlink_path != resolved_path {
                                format!(
                                    "Using existing directory: {} -> {}",
                                    symlink_path.display(),
                                    resolved_path.display()
                                )
                            } else {
                                format!("Using existing directory: {}", resolved_path.display())
                            }
                        });
                        Some(resolved_path)
                    }
                    Err(e) => profile_guard.modify(|| {
                        eprintln!(
                            "Warning: failed to resolve directory {}: {}",
                            symlink_path.display(),
                            e
                        );

                        // Remove the insecure , we'll create a new temp directory below.
                        verbose.print(|| {
                            format!("Removing insecure symlink: {}", symlink_path.display())
                        });
                        fs::remove_file(&symlink_path)?;
                        Ok(None)
                    })?,
                }
            } else {
                if symlink_path.is_symlink() {
                    profile_guard.modify(|| {
                        // The target is missing.
                        verbose.print(|| {
                            format!("Removing outdated symlink: {}", symlink_path.display())
                        });
                        fs::remove_file(&symlink_path)?;
                        Ok(())
                    })?;
                }
                None
            };

            let target_path = match target_path {
                Some(path) => path,
                None => profile_guard.modify(|| {
                    // Create a new directory for this workspace environment type.
                    let target_path = workspace.create_directory(
                        resolved_profile.name,
                        symlink_original_env,
                        read_env,
                    )?;
                    verbose.print(|| {
                        format!(
                            "Creating symlink {} -> {}",
                            symlink_path.display(),
                            target_path.display()
                        )
                    });
                    symlink(&target_path, &symlink_path)?;
                    Ok(target_path)
                })?,
            };

            env_vars.insert(
                env_var.to_string(),
                target_path.to_string_lossy().to_string(),
            );
            verbose.print(|| format!("Set {}={}", env_var, target_path.display()));
        }

        for workspace in AbsoluteWorkspace::new(resolved_profile.profile) {
            let env_var = workspace.env_var();
            let target_path = workspace.target_path();

            env_vars.insert(
                env_var.to_string(),
                target_path.to_string_lossy().to_string(),
            );
            verbose.print(|| format!("Set {}={}", env_var, target_path.display()));
        }

        Ok(env_vars)
    }

    pub fn update_ruleset(
        &self,
        ruleset: landlock::RulesetCreated,
        verbose: &Verbose,
    ) -> Result<landlock::RulesetCreated, IslandError> {
        let landlock_abi = ABI::V6;

        for (env_var, path) in &self.env_vars {
            verbose.print(|| format!("Allowing access to workspace {}: {}", env_var, path));
        }

        // Add all workspace directories with full filesystem access permissions
        let rules = path_beneath_rules(self.env_vars.values(), AccessFs::from_all(landlock_abi));
        Ok(ruleset.add_rules(rules)?)
    }

    pub fn update_environment<'a>(
        &'a self,
        env_vars: &mut BTreeMap<&'a String, &'a String>,
        verbose: &Verbose,
    ) {
        for (var_name, var_value) in &self.env_vars {
            verbose.print(|| format!("Setting workspace {}={}", var_name, var_value));
            if env_vars.insert(var_name, var_value).is_some() {
                eprintln!(
                    "Warning: Overwriting existing environment variable {} with workspace value",
                    var_name
                );
            }
        }
    }
}
