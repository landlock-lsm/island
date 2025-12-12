// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::{
    context::{ContextEntry, ContextSet},
    lock::{ExclusiveLock, ProfileGuard, ProfileLock, SharedLock, SharedLockError},
    workspace::WorkspaceManager,
    IslandError, Verbose,
};
use landlockconfig::{Config, ConfigFormat, ParseDirectoryError, ResolveError, ResolvedConfig};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const ISLAND_DEFAULT_CONFIG_BASE_NAME: &str = "island-default-base.toml";
pub const ISLAND_DEFAULT_CONFIG_BASE_CONTENT: &str =
    include_str!("../assets/landlock/island-default-base.toml");

pub const ISLAND_CUSTOM_CONFIG_NAME: &str = "island-custom.toml";
pub const ISLAND_CUSTOM_CONFIG_HEADER_CONTENT: &str =
    include_str!("../assets/landlock/island-custom-header.toml");

fn check_profile_default<L>(profile_guard: &ProfileGuard<L>) -> io::Result<Option<PathBuf>>
where
    L: ProfileLock,
{
    let default_base_path = profile_guard
        .path_landlock()
        .join(ISLAND_DEFAULT_CONFIG_BASE_NAME);

    if default_base_path.exists() {
        let content = fs::read_to_string(&default_base_path)?;
        if content != ISLAND_DEFAULT_CONFIG_BASE_CONTENT {
            return Ok(Some(default_base_path));
        }
    }
    Ok(None)
}

#[derive(Debug, Error)]
pub enum LandlockConfigErrorKind {
    #[error("Failed to parse Landlock configuration: {source}")]
    Parse {
        #[source]
        source: ParseDirectoryError,
    },
    #[error("Failed to resolve Landlock configuration: {source}")]
    Resolve {
        #[source]
        source: ResolveError,
    },
}

#[derive(Debug, Error)]
#[error("Profile '{profile_name}' from {landlock_dir}: {kind}")]
pub struct LandlockConfigError {
    pub profile_name: String,
    pub landlock_dir: PathBuf,
    #[source]
    pub kind: LandlockConfigErrorKind,
}

#[derive(Debug, Deserialize, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Env {
    pub name: String,
    pub literal: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedProfile<'a> {
    pub name: &'a str,
    pub profile: &'a Profile,
    pub context: Option<&'a ContextEntry>,
    pub config: ResolvedConfig,
    pub env_vars: &'a BTreeSet<Env>,
    pub workspace: bool,
}

impl<'a> ResolvedProfile<'a> {
    fn new<F, E>(
        name: &'a str,
        profile: &'a Profile,
        load_config: F,
        context: Option<&'a ContextEntry>,
    ) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        Ok(Self {
            name,
            profile,
            context,
            config: load_config(name).map_err(|e| e.into())?,
            env_vars: &profile.env_vars,
            workspace: profile.workspace,
        })
    }

    pub fn workspace_manager<E>(
        &'a self,
        island_config: &'a IslandConfig,
        verbose: &Verbose,
        read_env: E,
    ) -> Result<WorkspaceManager, IslandError>
    where
        E: Fn(&str) -> Result<String, env::VarError>,
    {
        WorkspaceManager::new(self, island_config, verbose, read_env)
    }
}

/// The greatest has the more tailored context, otherwise fall back to
/// lexicographic ordering of the profile's name.
impl Ord for ResolvedProfile<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // This ordering is efficient and enough to ensure a consistent ordering
        // considering elements returned by resolve_profiles_by_path().  There
        // is no need to rely on compare_precedence().
        self.context
            .cmp(&other.context)
            .then_with(|| self.name.cmp(other.name))
    }
}

impl PartialOrd for ResolvedProfile<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),
    #[error("no context found for current directory: {cwd}")]
    NoContextForDirectory { cwd: String },
    #[error("profile \"{name}\" not found in configuration")]
    ProfileNotFound { name: String },
    #[error("Unable to find the home configuration directory: empty $XDG_CONFIG_HOME and $HOME")]
    UnknownHomeConfig,
    #[error("failed to list profiles in {path}: {source}")]
    ProfilesDirectory {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    LandlockConfig(#[from] LandlockConfigError),
}

// Handle empty profile files.  This is useful to validate a profile without context.
#[derive(Debug, Deserialize)]
struct ProfileConfig {
    #[serde(rename = "context")]
    contexts: Option<Vec<TomlContextEntry>>,
    env: Option<Vec<Env>>,
    workspace: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TomlContextEntry {
    pub when_beneath: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub contexts: ContextSet,
    pub env_vars: BTreeSet<Env>,
    pub workspace: bool,
}

// Profile names should be trusted, but let's enforce some basic sanity checks
// to avoid usability issues.
pub fn is_profile_name_valid<S>(name: S) -> bool
where
    S: AsRef<OsStr>,
{
    let name = name.as_ref();

    // Check for path separators and directory traversal.
    if Path::new(name).file_name().is_none_or(|n| n != name) {
        return false;
    }

    let name_str = name.to_string_lossy();

    // Prevent leading dots (hidden), dashes (CLI flags), and path expansion.
    if name_str.starts_with('.') || name_str.starts_with('-') || name_str.starts_with('~') {
        return false;
    }

    // Prevent leading and trailing whitespace.
    if name_str.chars().next().is_some_and(char::is_whitespace)
        || name_str.chars().last().is_some_and(char::is_whitespace)
    {
        return false;
    }

    // Check for common CLI special characters.
    name_str.chars().all(|c| {
        c != '$' && c != '*' && c != '|' && c != '&' && c != ';' && c != '<' && c != '>' && c != '`'
    })
}

type Profiles = BTreeMap<String, Profile>;

#[derive(Debug, Default)]
pub struct IslandConfig {
    profiles: Profiles,
    profiles_dir: PathBuf,
}

impl IslandConfig {
    /// Creates a new IslandConfig by loading configuration from ~/.config/island/
    ///
    /// Configuration layout in ~/.config/island/profiles/<profile_name>/
    /// - profile.toml: Contains contexts for this profile
    /// - landlock/: Contains Landlock configuration
    ///
    /// The profile name is derived from the directory name.
    pub fn new<E>(read_env: E) -> Result<Self, ConfigError>
    where
        E: Fn(&str) -> Result<String, env::VarError>,
    {
        let mut config = Self {
            profiles_dir: Self::config_dir(read_env)?.join("profiles"),
            ..Default::default()
        };
        let profiles_entries = fs::read_dir(&config.profiles_dir).map_err(|source| {
            ConfigError::ProfilesDirectory {
                path: config.profiles_dir.display().to_string(),
                source,
            }
        })?;

        for entry in profiles_entries {
            let entry = entry?;
            let file_name = entry.file_name();
            if !is_profile_name_valid(&file_name) {
                continue;
            }
            if entry.file_type()?.is_dir() {
                let profile_name = file_name.to_string_lossy().to_string();
                let island_toml_path = entry.path().join("profile.toml");

                if island_toml_path.exists() {
                    let profile = config.parse_profile_config(
                        &fs::read_to_string(&island_toml_path)?,
                        &profile_name,
                        |path| path.canonicalize(),
                    )?;

                    // Ignore potential race conditions when listing the content
                    // of a directory and it returns the same entry several
                    // times. In this case, just ignore the previous similar
                    // one(s).
                    config.profiles.insert(profile_name, profile);
                }
            }
        }

        Ok(config)
    }

    pub fn config_dir<E>(read_env: E) -> Result<PathBuf, ConfigError>
    where
        E: Fn(&str) -> Result<String, env::VarError>,
    {
        let home_config = if let Ok(c) = read_env("XDG_CONFIG_HOME") {
            c.into()
        } else if let Ok(h) = read_env("HOME") {
            PathBuf::from(h).join(".config")
        } else {
            return Err(ConfigError::UnknownHomeConfig);
        };
        Ok(home_config.join("island"))
    }

    fn parse_profile_config<F>(
        &self,
        content: &str,
        profile_name: &str,
        canonicalize_path: F,
    ) -> Result<Profile, ConfigError>
    where
        F: Fn(&Path) -> io::Result<PathBuf>,
    {
        let mut profile = Profile::default();
        let cfg = toml::from_str::<ProfileConfig>(content)?;

        for cfg_context in cfg.contexts.unwrap_or_default() {
            // Canonicalize the when_beneath path to resolve symlinks and ignore
            // contexts with non-existing directories.
            let context = match canonicalize_path(&cfg_context.when_beneath) {
                Ok(p) => ContextEntry {
                    when_beneath: Some(p),
                },
                Err(e) => {
                    eprintln!(
                            "Warning: ignoring context for profile \"{}\" because of error regarding directory \"{}\": {}",
                            profile_name,
                            cfg_context.when_beneath.display(),
                            e
                        );
                    continue;
                }
            };

            if let Some(message) = profile.contexts.insert(context).warning(profile_name) {
                eprintln!("Warning: {}", message);
            }
        }

        profile.env_vars.extend(cfg.env.unwrap_or_default());

        profile.workspace = cfg.workspace.unwrap_or(true);

        Ok(profile)
    }

    /// Resolves all contexts that match the provided path in hierarchical order.
    ///
    /// `canonicalized_path` should already have been canonicalized with `canonicalize()`.
    /// The closure `load_config` is called for each matching context to load its configuration.
    ///
    /// Returns resolved profiles tied to the first matching context, sorted by profile names.
    pub fn resolve_profiles_by_path<P, F, E>(
        &self,
        canonicalized_path: P,
        load_config: F,
    ) -> Result<BTreeSet<ResolvedProfile<'_>>, ConfigError>
    where
        P: AsRef<Path>,
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        let canonicalized_path = canonicalized_path.as_ref();

        // Consistently store sorted paths for scope ordering and determinism.
        let resolved: Result<BTreeSet<_>, ConfigError> = self
            .profiles
            .iter()
            .filter_map(|(profile_name, profile)| {
                // Find the first matching context
                profile
                    .contexts
                    .iter()
                    .find(|context| {
                        matches!(
                            context.when_beneath.as_ref(),
                            Some(when_beneath) if canonicalized_path.starts_with(when_beneath)
                        )
                    })
                    .map(|context| {
                        ResolvedProfile::new(profile_name, profile, &load_config, Some(context))
                    })
            })
            .collect();

        let resolved = resolved?;
        if resolved.is_empty() {
            return Err(ConfigError::NoContextForDirectory {
                cwd: canonicalized_path.display().to_string(),
            });
        }

        Ok(resolved)
    }

    pub fn guard_profile<L>(&self, profile_name: &str) -> io::Result<ProfileGuard<L>>
    where
        L: ProfileLock,
    {
        let profile_dir = self.profiles_dir.join(profile_name);
        ProfileGuard::<L>::new(profile_dir)
    }

    /// Loads landlock configuration for a profile by name.
    pub fn load_landlock_config(&self, profile_name: &str) -> Result<ResolvedConfig, ConfigError> {
        let profile_guard = self.guard_profile::<SharedLock<ConfigError>>(profile_name)?;

        if let Some(path) = check_profile_default(&profile_guard)? {
            eprintln!(
                "Warning: {} is not the latest version. \
                Run \"island update\" to fix it.",
                path.display()
            );
        }

        let landlock_dir = profile_guard.path_landlock();

        // Parse the configuration with profile-specific error context.
        Ok(Config::parse_directory(&landlock_dir, ConfigFormat::Toml)
            .map_err(|source| LandlockConfigError {
                profile_name: profile_name.to_string(),
                landlock_dir: landlock_dir.clone(),
                kind: LandlockConfigErrorKind::Parse { source },
            })?
            .resolve()
            .map_err(|source| LandlockConfigError {
                profile_name: profile_name.to_string(),
                landlock_dir,
                kind: LandlockConfigErrorKind::Resolve { source },
            })?)
    }

    // Even if this command is dedicated to change a configuration file, this
    // might not be necessary, and we should favor non-exclusive operations
    // related to profiles.  This is why we first try with a shared lock, and we
    // may read two times the same configuration file.
    pub fn update_profile_default(&self, profile_name: &str) -> Result<(), IslandError> {
        // First pass: check if changes are needed with a shared lock.
        match self.update_profile_default_lock::<SharedLock<IslandError>>(profile_name) {
            Ok(()) => Ok(()),
            Err(SharedLockError::Inner(e)) => Err(e),
            Err(SharedLockError::NeedsUpdate) => {
                // Second pass: perform changes with an exclusive lock.
                self.update_profile_default_lock::<ExclusiveLock<IslandError>>(profile_name)
            }
        }
    }

    fn update_profile_default_lock<L>(&self, profile_name: &str) -> Result<(), L::Error>
    where
        L: ProfileLock,
        L::Error: From<IslandError> + From<io::Error>,
    {
        let profile_guard = self.guard_profile::<L>(profile_name)?;

        if let Some(path) = check_profile_default(&profile_guard)? {
            profile_guard.modify(|| {
                fs::write(&path, ISLAND_DEFAULT_CONFIG_BASE_CONTENT)?;
                println!("Updated {}", path.display());
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Resolves profiles by explicit profile names.
    /// The closure `load_config` is called for each profile name to load its configuration.
    pub fn resolve_profiles_by_names<S, F, E>(
        &self,
        profile_names: &[S],
        load_config: F,
    ) -> Result<Vec<ResolvedProfile<'_>>, ConfigError>
    where
        S: AsRef<str>,
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        profile_names
            .iter()
            .map(|profile_name| {
                // Get the actual key from contexts to ensure lifetime.
                let (profile_key, profile) = self
                    .profiles
                    .get_key_value(profile_name.as_ref())
                    .ok_or_else(|| ConfigError::ProfileNotFound {
                        name: profile_name.as_ref().to_string(),
                    })?;

                ResolvedProfile::new(
                    profile_key,
                    profile,
                    &load_config,
                    // Context is always None since we resolved by name.
                    None,
                )
            })
            .collect()
    }

    pub fn profile_names(&self) -> impl Iterator<Item = &String> + '_ {
        self.profiles.keys()
    }
}

pub fn generate_path_beneath_rule(allowed_access: &[String], parent: &[String]) -> String {
    let parent_paths = parent
        .iter()
        .map(|path| format!("    {}", toml::Value::String(path.clone())))
        .collect::<Vec<_>>()
        .join(",\n")
        + ",";

    let allowed_accesses = allowed_access
        .iter()
        .map(|access| format!("{}", toml::Value::String(access.clone())))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "[[path_beneath]]\nallowed_access = [{}]\nparent = [\n{}\n]\n",
        allowed_accesses, parent_paths
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;

    fn create_test_config_with_profiles<I>(profiles_data: I) -> IslandConfig
    where
        I: IntoIterator<Item = (&'static str, &'static str)>,
    {
        let mut config = IslandConfig {
            profiles_dir: "/test/config/profiles/dir".into(),
            ..Default::default()
        };

        for (profile_name, toml_content) in profiles_data {
            let profile = config
                .parse_profile_config(
                    toml_content,
                    profile_name,
                    // Pure function, independent from the filesystem (i.e. do not check if the path exists).
                    |p| Ok(p.to_path_buf()),
                )
                .unwrap();

            // Ensure profile names are unique - insert should return None (no previous value)
            assert!(
                config
                    .profiles
                    .insert(profile_name.to_string(), profile)
                    .is_none(),
                "Duplicate profile name in test data: {}",
                profile_name
            );
        }

        config
    }

    fn create_test_config() -> IslandConfig {
        let profiles_data = [
            (
                "home",
                r#"
[[context]]
when_beneath = "/home/user"
"#,
            ),
            (
                "projects",
                r#"
[[context]]
when_beneath = "/home/user/projects"
"#,
            ),
            (
                "work1",
                r#"
[[context]]
when_beneath = "/home/user/projects/work1"
"#,
            ),
            ("standalone", ""),
        ];

        create_test_config_with_profiles(profiles_data)
    }

    #[test]
    fn test_empty_profile() {
        create_test_config_with_profiles([("empty", "")]);
    }

    #[test]
    fn test_resolve_profiles_map_error() {
        let config = create_test_config();
        let matches = RefCell::new(0);

        let result = config.resolve_profiles_by_path("/home/user/projects/work1/foo", |_| {
            *matches.borrow_mut() += 1;
            Err(ConfigError::ProfileNotFound {
                name: "test".to_string(),
            })
        });

        // The closure should be called for the first matching profile (home).
        assert_eq!(1, *matches.borrow());
        assert!(matches!(
            result,
            Err(ConfigError::ProfileNotFound { name }) if name == "test"
        ));
    }

    pub fn create_mock_resolved_config() -> ResolvedConfig {
        let mini = r#"
[[ruleset]]
scoped = ["signal"]
"#;
        Config::parse_toml(mini).unwrap().resolve().unwrap()
    }

    #[test]
    fn test_resolve_profiles_matches() {
        let config = create_test_config();

        let result = config
            .resolve_profiles_by_path(
                "/home/user/projects",
                |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
            )
            .unwrap();

        let mut resolved_iter = result.iter();

        assert!(matches!(
            resolved_iter.next().unwrap(),
            ResolvedProfile { name: "home", .. },
        ));
        assert!(matches!(
            resolved_iter.next().unwrap(),
            ResolvedProfile {
                name: "projects",
                ..
            },
        ));
        assert_eq!(resolved_iter.next(), None);
    }

    #[test]
    fn test_resolve_profiles_single_match() {
        let config = create_test_config();

        // Test path that only matches one profile but returns an error.
        let result = config.resolve_profiles_by_path("/home/user/downloads", |_| {
            Err(ConfigError::ProfileNotFound {
                name: "test".to_string(),
            })
        });

        assert!(matches!(
            result,
            Err(ConfigError::ProfileNotFound { name }) if name == "test"));
    }

    #[test]
    fn test_resolve_profiles_no_match() {
        let config = create_test_config();

        // Test path that matches no profiles.
        let result = config.resolve_profiles_by_path(
            "/home/bob/projects",
            |_| -> Result<ResolvedConfig, ConfigError> {
                panic!("Closure should not be called when no profiles match")
            },
        );

        assert!(
            matches!(result, Err(ConfigError::NoContextForDirectory { cwd }) if cwd == "/home/bob/projects")
        );
    }

    #[test]
    fn test_resolve_profiles_by_names_with_optional_when_beneath() {
        let config = create_test_config();

        // Test resolving profiles without when_beneath using resolve_profiles_by_names.
        let result = config.resolve_profiles_by_names(
            &["standalone".to_string()],
            |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
        );

        assert!(matches!(
            result.as_deref(),
            Ok([ResolvedProfile { name, context: None, .. }]) if *name == "standalone"
        ));

        // Test resolving mixed profiles (with and without context,
        // but none returned since resolved by name).
        let result = config.resolve_profiles_by_names(
            &["home".to_string(), "standalone".to_string()],
            |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
        );

        assert!(matches!(
            result.as_deref(),
            Ok([
                ResolvedProfile { name, context: None, .. },
                ResolvedProfile { name: name2, context: None, .. }
            ]) if *name == "home" && *name2 == "standalone"
        ));
    }

    #[test]
    fn test_parse_config_dup_and_sorted() {
        // Test data with duplicates and unsorted entries.
        let profiles_data = [
            (
                "b",
                r#"
[[context]]
when_beneath = "/foo"

[[context]]
when_beneath = "/foo"

[[context]]
when_beneath = "/bar"
"#,
            ),
            (
                "a",
                r#"
[[context]]
when_beneath = "/foo"

[[context]]
when_beneath = "/foo"
"#,
            ),
        ];

        let config = create_test_config_with_profiles(profiles_data);

        let mut profile_iter = config.profiles.iter();

        let profile = profile_iter.next().unwrap();
        assert_eq!(profile.0, "a");

        // Sorted by profile's name and when_beneath.
        let mut entry_iter = profile.1.contexts.iter();
        assert_eq!(
            entry_iter.next(),
            Some(&ContextEntry {
                when_beneath: Some("/foo".into()),
            })
        );
        assert_eq!(entry_iter.next(), None);

        let profile = profile_iter.next().unwrap();
        assert_eq!(profile.0, "b");

        // Sorted by profile's name and when_beneath.
        let mut entry_iter = profile.1.contexts.iter();
        assert_eq!(
            entry_iter.next(),
            Some(&ContextEntry {
                when_beneath: Some("/bar".into()),
            })
        );
        assert_eq!(
            entry_iter.next(),
            Some(&ContextEntry {
                when_beneath: Some("/foo".into()),
            })
        );
        assert_eq!(entry_iter.next(), None);
        assert_eq!(profile_iter.next(), None);

        // Check duplicate when_beneath with similar profile's name.
        let mut profile_iter = config
            .resolve_profiles_by_path("/foo", |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap()
            .into_iter();
        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "a",
                context: Some(&ContextEntry {
                    when_beneath: Some(ref path),
                }),
                ..
            } if path == &PathBuf::from("/foo")
        ));
        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "b",
                context: Some(&ContextEntry {
                    when_beneath: Some(ref path),
                }),
                ..
            } if path == &PathBuf::from("/foo")
        ));
        assert_eq!(profile_iter.next(), None);

        // Check duplicate profile's name with different when_beneath.
        let mut profile_iter = config
            .resolve_profiles_by_names(&["b"], |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap()
            .into_iter();
        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "b",
                context: None,
                ..
            }
        ));
        assert_eq!(profile_iter.next(), None);
    }

    pub fn create_resolved_profile<'a>(
        name: &'a str,
        context: Option<&'a ContextEntry>,
        profile: &'a Profile,
    ) -> ResolvedProfile<'a> {
        static EMPTY_BTREE_SET: BTreeSet<Env> = BTreeSet::new();

        ResolvedProfile {
            name,
            profile,
            context,
            config: create_mock_resolved_config(),
            env_vars: &EMPTY_BTREE_SET,
            workspace: true,
        }
    }

    #[test]
    #[allow(clippy::nonminimal_bool)]
    fn test_resolved_profile_ordering() {
        let source_profile = Profile::default();

        // Empty contexts.
        let ctx_beneath_none = Some(&ContextEntry { when_beneath: None });
        let profile1 = create_resolved_profile("a", ctx_beneath_none, &source_profile);
        let profile2 = create_resolved_profile("a", ctx_beneath_none, &source_profile);
        assert!(!(profile1 < profile2));
        assert!(profile1 == profile2);
        assert!(!(profile1 > profile2));

        // Fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_none, &source_profile);
        let profile2 = create_resolved_profile("b", ctx_beneath_none, &source_profile);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));

        // Empty vs. non-empty context.
        let ctx_beneath_foo = Some(&ContextEntry {
            when_beneath: Some("/foo".into()),
        });
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("a", ctx_beneath_none, &source_profile);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Do not fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("b", ctx_beneath_none, &source_profile);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Context with sibling paths.
        let ctx_beneath_bar = Some(&ContextEntry {
            when_beneath: Some("/bar".into()),
        });
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("a", ctx_beneath_bar, &source_profile);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Do not fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("b", ctx_beneath_bar, &source_profile);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Context with nested path.
        let ctx_beneath_foo_bar = Some(&ContextEntry {
            when_beneath: Some("/foo/bar".into()),
        });
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("a", ctx_beneath_foo_bar, &source_profile);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));

        // Do not fall back to lexicographic order.
        let profile1 = create_resolved_profile("b", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("a", ctx_beneath_foo_bar, &source_profile);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));

        // Context with same path.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        assert!(!(profile1 < profile2));
        assert!(profile1 == profile2);
        assert!(!(profile1 > profile2));

        // Fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo, &source_profile);
        let profile2 = create_resolved_profile("b", ctx_beneath_foo, &source_profile);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));
    }

    #[test]
    fn test_resolved_profile_sorted() {
        let source_profile = Profile::default();

        let ctx_beneath_none = Some(&ContextEntry { when_beneath: None });
        let ctx_beneath_foo = Some(&ContextEntry {
            when_beneath: Some("/foo".into()),
        });
        let ctx_beneath_bar = Some(&ContextEntry {
            when_beneath: Some("/bar".into()),
        });
        let ctx_beneath_foo_bar = Some(&ContextEntry {
            when_beneath: Some("/foo/bar".into()),
        });

        let sorted = [
            create_resolved_profile("a", ctx_beneath_none, &source_profile),
            create_resolved_profile("b", ctx_beneath_none, &source_profile),
            create_resolved_profile("a", ctx_beneath_bar, &source_profile),
            create_resolved_profile("b", ctx_beneath_bar, &source_profile),
            create_resolved_profile("a", ctx_beneath_foo, &source_profile),
            create_resolved_profile("b", ctx_beneath_foo, &source_profile),
            create_resolved_profile("a", ctx_beneath_foo_bar, &source_profile),
            create_resolved_profile("b", ctx_beneath_foo_bar, &source_profile),
        ];
        // Create a BTreeSet from unsorted and duplicated profiles.
        let set: BTreeSet<_> = sorted.iter().rev().chain(sorted.iter()).collect();

        // Check growing order.
        assert_eq!(sorted.len(), set.len());
        for (i, profile) in set.into_iter().enumerate() {
            assert_eq!(*profile, sorted[i]);
        }
    }

    fn create_test_config_for_ordering() -> IslandConfig {
        let profiles_data = [
            (
                "d",
                r#"
[[context]]
when_beneath = "/foo"
"#,
            ),
            (
                "c",
                r#"
[[context]]
when_beneath = "/foo"
"#,
            ),
            (
                "b",
                r#"
[[context]]
when_beneath = "/"
"#,
            ),
            (
                "a",
                r#"
[[context]]
when_beneath = "/foo/bar"
"#,
            ),
        ];
        create_test_config_with_profiles(profiles_data)
    }

    #[test]
    fn test_resolve_profile_by_path_sorted() {
        let config = create_test_config_for_ordering();

        let resolved_profiles = config
            .resolve_profiles_by_path("/foo/bar", |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap();
        let mut profile_iter = resolved_profiles.iter();

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "b",
                context: Some(&ContextEntry {
                    when_beneath: Some(ref path),
                }),
                ..
            } if path == &PathBuf::from("/")
        ));

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "c",
                context: Some(&ContextEntry {
                    when_beneath: Some(ref path),
                }),
                ..
            } if path == &PathBuf::from("/foo")
        ));

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "d",
                context: Some(&ContextEntry {
                    when_beneath: Some(ref path),
                }),
                ..
            } if path == &PathBuf::from("/foo")
        ));

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "a",
                context: Some(&ContextEntry {
                    when_beneath: Some(ref path),
                }),
                ..
            } if path == &PathBuf::from("/foo/bar")
        ));

        assert_eq!(profile_iter.next(), None);
    }

    #[test]
    fn test_resolve_profile_by_name_unsorted() {
        let config = create_test_config_for_ordering();
        let name_order = ["d", "c", "a", "b"];
        let resolved_profiles = config
            .resolve_profiles_by_names(&name_order, |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap();

        let mut profile_iter = resolved_profiles.iter();
        for n in &name_order {
            assert!(matches!(
                profile_iter.next().unwrap(),
                ResolvedProfile {
                    name,
                    context: None,
                    ..
                } if name == n
            ));
        }
        assert_eq!(profile_iter.next(), None);
    }

    #[test]
    fn test_parse_config_env() {
        let profiles_data = [
            (
                "with context",
                r#"
[[context]]
when_beneath = "/foo"

[[env]]
name = "FOO"
literal = "/tmp/foo"

[[env]]
name = "BAR"
literal = "/tmp/bar"
"#,
            ),
            (
                "without context",
                r#"
[[env]]
name = "BAR"
literal = "/tmp/bar2"
"#,
            ),
        ];

        let config = create_test_config_with_profiles(profiles_data);
        let resolved_profiles = config
            .resolve_profiles_by_path("/foo/bar", |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap();
        let mut profile_iter = resolved_profiles.iter();

        let profile = profile_iter.next().unwrap();
        assert!(matches!(
            profile,
            ResolvedProfile {
            name: "with context",
            context: Some(&ContextEntry {
                when_beneath: Some(ref path),
            }),
            ..
            } if path == &PathBuf::from("/foo")
        ));
        assert_eq!(
            profile.env_vars,
            &[
                Env {
                    name: "FOO".to_string(),
                    literal: "/tmp/foo".to_string(),
                },
                Env {
                    name: "BAR".to_string(),
                    literal: "/tmp/bar".to_string(),
                }
            ]
            .into()
        );
        assert_eq!(profile_iter.next(), None);
    }

    #[test]
    fn test_parse_config_workspace() {
        let profiles_data = [
            (
                "foo",
                r#"
workspace = true
"#,
            ),
            (
                "bar",
                r#"
workspace = false
"#,
            ),
            ("baz", ""),
        ];

        let config = create_test_config_with_profiles(profiles_data);
        let resolved_profiles = config
            .resolve_profiles_by_names(
                &["foo", "bar", "baz"],
                |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
            )
            .unwrap();
        let mut profile_iter = resolved_profiles.iter();

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "foo",
                workspace: true,
                ..
            }
        ));

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "bar",
                workspace: false,
                ..
            }
        ));

        assert!(matches!(
            profile_iter.next().unwrap(),
            ResolvedProfile {
                name: "baz",
                // Default value.
                workspace: true,
                ..
            }
        ));

        assert_eq!(profile_iter.next(), None);
    }

    #[test]
    fn test_is_profile_name_valid() {
        assert!(is_profile_name_valid("foo"));
        assert!(is_profile_name_valid("foo-bar"));
        assert!(is_profile_name_valid("foo_bar"));
        assert!(is_profile_name_valid("foo.bar"));
        assert!(is_profile_name_valid("foo bar"));
        assert!(is_profile_name_valid("foo@bar"));
        assert!(is_profile_name_valid("foo+bar"));

        assert!(!is_profile_name_valid("."));
        assert!(!is_profile_name_valid(".."));
        assert!(!is_profile_name_valid(".hidden"));
        assert!(!is_profile_name_valid("-flag"));
        assert!(!is_profile_name_valid("foo/bar"));
        assert!(!is_profile_name_valid("/foo"));
        assert!(!is_profile_name_valid("foo/"));
        assert!(!is_profile_name_valid("~foo"));
        assert!(!is_profile_name_valid(""));
        assert!(!is_profile_name_valid(" "));
        assert!(!is_profile_name_valid(" foo"));
        assert!(!is_profile_name_valid("\tfoo"));
        assert!(!is_profile_name_valid("foo "));
        assert!(!is_profile_name_valid("foo\t"));
        assert!(!is_profile_name_valid("\n"));
        assert!(!is_profile_name_valid("foo$bar"));
        assert!(!is_profile_name_valid("foo*bar"));
        assert!(!is_profile_name_valid("foo|bar"));
        assert!(!is_profile_name_valid("foo&bar"));
        assert!(!is_profile_name_valid("foo;bar"));
        assert!(!is_profile_name_valid("foo<bar"));
        assert!(!is_profile_name_valid("foo>bar"));
        assert!(!is_profile_name_valid("foo`bar"));
    }
}
