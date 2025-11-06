// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::context::{ContextEntry, ContextSet};
use landlockconfig::{Config, ConfigFormat, ParseDirectoryError, ResolveError, ResolvedConfig};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProfileErrorKind {
    #[error("Failed to parse configuration: {source}")]
    Parse {
        #[source]
        source: ParseDirectoryError,
    },
    #[error("Failed to resolve configuration: {source}")]
    Resolve {
        #[source]
        source: ResolveError,
    },
}

#[derive(Debug, Error)]
#[error("Profile '{profile_name}' from {profile_dir}: {kind}")]
pub struct ProfileError {
    pub profile_name: String,
    pub profile_dir: PathBuf,
    #[source]
    pub kind: ProfileErrorKind,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedProfile<'a> {
    pub name: &'a str,
    pub context: Option<&'a ContextEntry>,
    pub config: ResolvedConfig,
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
    Io(#[from] std::io::Error),
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
        source: std::io::Error,
    },
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

// Handle empty profile files.  This is useful to validate a profile without context.
#[derive(Debug, Deserialize)]
struct ProfileConfig {
    #[serde(rename = "context")]
    contexts: Option<Vec<TomlContextEntry>>,
}

#[derive(Debug, Deserialize)]
struct TomlContextEntry {
    pub when_beneath: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub contexts: ContextSet,
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
    pub fn new() -> Result<Self, ConfigError> {
        let mut config = Self {
            profiles_dir: Self::get_config_dir()?.join("profiles"),
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
            if entry.file_type()?.is_dir() {
                let profile_name = entry.file_name().to_string_lossy().to_string();
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

    fn get_config_dir() -> Result<PathBuf, ConfigError> {
        let home_config = if let Ok(c) = std::env::var("XDG_CONFIG_HOME") {
            c.into()
        } else if let Ok(h) = std::env::var("HOME") {
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
        F: Fn(&Path) -> std::io::Result<PathBuf>,
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
                        Ok(ResolvedProfile {
                            name: profile_name,
                            context: Some(context),
                            config: load_config(profile_name).map_err(|e| e.into())?,
                        })
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

    /// Loads landlock configuration for a profile by name.
    pub fn load_landlock_config(&self, profile_name: &str) -> Result<ResolvedConfig, ProfileError> {
        let profile_dir = self.profiles_dir.join(profile_name).join("landlock");

        // Parse the configuration with profile-specific error context.
        Config::parse_directory(&profile_dir, ConfigFormat::Toml)
            .map_err(|source| ProfileError {
                profile_name: profile_name.to_string(),
                profile_dir: profile_dir.clone(),
                kind: ProfileErrorKind::Parse { source },
            })?
            .resolve()
            .map_err(|source| ProfileError {
                profile_name: profile_name.to_string(),
                profile_dir: profile_dir.clone(),
                kind: ProfileErrorKind::Resolve { source },
            })
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
                let profile_key = self
                    .profiles
                    .get_key_value(profile_name.as_ref())
                    .ok_or_else(|| ConfigError::ProfileNotFound {
                        name: profile_name.as_ref().to_string(),
                    })?
                    .0;

                Ok(ResolvedProfile {
                    name: profile_key,
                    // Context is always None since we resolved by name.
                    context: None,
                    config: load_config(profile_key).map_err(|e| e.into())?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
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

    fn create_mock_resolved_config() -> ResolvedConfig {
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

    fn create_resolved_profile<'a>(
        name: &'a str,
        context: Option<&'a ContextEntry>,
    ) -> ResolvedProfile<'a> {
        ResolvedProfile {
            name,
            context,
            config: create_mock_resolved_config(),
        }
    }

    #[test]
    #[allow(clippy::nonminimal_bool)]
    fn test_resolved_profile_ordering() {
        // Empty contexts.
        let ctx_beneath_none = Some(&ContextEntry { when_beneath: None });
        let profile1 = create_resolved_profile("a", ctx_beneath_none);
        let profile2 = create_resolved_profile("a", ctx_beneath_none);
        assert!(!(profile1 < profile2));
        assert!(profile1 == profile2);
        assert!(!(profile1 > profile2));

        // Fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_none);
        let profile2 = create_resolved_profile("b", ctx_beneath_none);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));

        // Empty vs. non-empty context.
        let ctx_beneath_foo = Some(&ContextEntry {
            when_beneath: Some("/foo".into()),
        });
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("a", ctx_beneath_none);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Do not fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("b", ctx_beneath_none);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Context with sibling paths.
        let ctx_beneath_bar = Some(&ContextEntry {
            when_beneath: Some("/bar".into()),
        });
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("a", ctx_beneath_bar);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Do not fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("b", ctx_beneath_bar);
        assert!(!(profile1 < profile2));
        assert!(!(profile1 == profile2));
        assert!(profile1 > profile2);

        // Context with nested path.
        let ctx_beneath_foo_bar = Some(&ContextEntry {
            when_beneath: Some("/foo/bar".into()),
        });
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("a", ctx_beneath_foo_bar);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));

        // Do not fall back to lexicographic order.
        let profile1 = create_resolved_profile("b", ctx_beneath_foo);
        let profile2 = create_resolved_profile("a", ctx_beneath_foo_bar);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));

        // Context with same path.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("a", ctx_beneath_foo);
        assert!(!(profile1 < profile2));
        assert!(profile1 == profile2);
        assert!(!(profile1 > profile2));

        // Fall back to lexicographic order.
        let profile1 = create_resolved_profile("a", ctx_beneath_foo);
        let profile2 = create_resolved_profile("b", ctx_beneath_foo);
        assert!(profile1 < profile2);
        assert!(!(profile1 == profile2));
        assert!(!(profile1 > profile2));
    }

    #[test]
    fn test_resolved_profile_sorted() {
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
            create_resolved_profile("a", ctx_beneath_none),
            create_resolved_profile("b", ctx_beneath_none),
            create_resolved_profile("a", ctx_beneath_bar),
            create_resolved_profile("b", ctx_beneath_bar),
            create_resolved_profile("a", ctx_beneath_foo),
            create_resolved_profile("b", ctx_beneath_foo),
            create_resolved_profile("a", ctx_beneath_foo_bar),
            create_resolved_profile("b", ctx_beneath_foo_bar),
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
}
