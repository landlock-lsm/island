// SPDX-License-Identifier: Apache-2.0 OR MIT

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
    pub entry: &'a ProfileEntry,
    pub config: ResolvedConfig,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),
    #[error("no profile found for current directory: {cwd}")]
    NoProfileForDirectory { cwd: String },
    #[error("profile \"{name}\" not found in configuration")]
    ProfileNotFound { name: String },
    #[error("Unable to find the home configuration directory: empty $XDG_CONFIG_HOME and $HOME")]
    UnknownHomeConfig,
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

#[derive(Debug, Deserialize)]
struct TomlConfig {
    #[serde(rename = "profile")]
    profiles: Vec<ProfileEntry>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileEntry {
    // TODO: Restrict name.
    pub name: String,
    pub when_beneath: Option<PathBuf>,
}

type Profiles = BTreeMap<String, BTreeSet<ProfileEntry>>;

#[derive(Debug)]
pub struct IslandConfig {
    profiles: Profiles,
    path: PathBuf,
}

impl IslandConfig {
    /// Load configuration from ~/.config/island/main.toml
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::get_config_dir()?;
        Ok(Self {
            profiles: Self::parse_config(&fs::read_to_string(path.join("main.toml"))?, |path| {
                path.canonicalize()
            })?,
            path,
        })
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

    fn parse_config<F>(content: &str, canonicalize_path: F) -> Result<Profiles, ConfigError>
    where
        F: Fn(&Path) -> std::io::Result<PathBuf>,
    {
        let mut profiles = BTreeMap::new();
        for mut profile in toml::from_str::<TomlConfig>(content)?.profiles {
            // Canonicalize the when_beneath path to resolve symlinks and ignore
            // profiles with non-existing directories.
            if let Some(when_beneath) = &profile.when_beneath {
                match canonicalize_path(when_beneath) {
                    Ok(p) => {
                        profile.when_beneath = Some(p);
                        profiles
                            .entry(profile.name.clone())
                            .or_insert_with(BTreeSet::new)
                            .insert(profile);
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: ignoring profile \"{}\" because of error regarding directory \"{}\": {}",
                            profile.name,
                            when_beneath.display(),
                            e
                        );
                    }
                }
            } else {
                profiles
                    .entry(profile.name.clone())
                    .or_insert_with(BTreeSet::new)
                    .insert(profile);
            }
        }
        Ok(profiles)
    }

    /// Resolve all profiles that match the provided path in hierarchical order.
    /// `canonicalized_path` should already have been canonicalized with `canonicalize()`.
    /// The closure `load_config` is called for each matching profile to load its configuration.
    /// Returns resolved profiles sorted from broadest scope to most specific scope.
    pub fn resolve_profiles_by_path<P, F, E>(
        &self,
        canonicalized_path: P,
        load_config: F,
    ) -> Result<Vec<ResolvedProfile<'_>>, ConfigError>
    where
        P: AsRef<Path>,
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        let canonicalized_path = canonicalized_path.as_ref();

        // Consistently store sorted paths for scope ordering and determinism.
        let resolved: Result<Vec<ResolvedProfile>, ConfigError> = self
            .profiles
            .values()
            .flat_map(|profile_set| profile_set.iter())
            .filter(|profile| {
                profile
                    .when_beneath
                    .as_ref()
                    .is_some_and(|when_beneath| canonicalized_path.starts_with(when_beneath))
            })
            .map(|profile| {
                Ok(ResolvedProfile {
                    entry: profile,
                    config: load_config(&profile.name).map_err(|e| e.into())?,
                })
            })
            .collect();

        let resolved = resolved?;
        if resolved.is_empty() {
            return Err(ConfigError::NoProfileForDirectory {
                cwd: canonicalized_path.display().to_string(),
            });
        }

        Ok(resolved)
    }

    /// Load landlock configuration for a profile by name.
    pub fn load_landlock_config(&self, profile_name: &str) -> Result<ResolvedConfig, ProfileError> {
        let profile_dir = self.get_landlock_directory(profile_name);

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

    fn get_landlock_directory(&self, profile_name: &str) -> PathBuf {
        self.path.join("landlock").join(profile_name)
    }

    /// Resolve profiles by explicit profile names.
    /// The closure `load_config` is called for each profile name to load its configuration.
    pub fn resolve_profiles_by_names<I, S, F, E>(
        &self,
        profile_names: I,
        load_config: F,
    ) -> Result<Vec<ResolvedProfile<'_>>, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        profile_names
            .into_iter()
            .try_fold(Vec::new(), |mut resolved, profile_name| {
                let profile_set = self.profiles.get(profile_name.as_ref()).ok_or_else(|| {
                    ConfigError::ProfileNotFound {
                        name: profile_name.as_ref().to_string(),
                    }
                })?;

                // Add all profiles with this name (there could be multiple with
                // different when_beneath).
                for profile in profile_set {
                    resolved.push(ResolvedProfile {
                        entry: profile,
                        config: load_config(&profile.name).map_err(|e| e.into())?,
                    });
                }
                Ok(resolved)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // Pure function, independent from the filesystem (i.e. do not check if the path exists).
    fn nocheck_path(path: &Path) -> std::io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }

    fn create_test_config() -> IslandConfig {
        let content = r#"
[[profile]]
name = "home"
when_beneath = "/home/user"

[[profile]]
name = "projects"
when_beneath = "/home/user/projects"

[[profile]]
name = "work1"
when_beneath = "/home/user/projects/work1"

[[profile]]
name = "standalone"
"#;

        IslandConfig {
            profiles: IslandConfig::parse_config(content, nocheck_path).unwrap(),
            path: Default::default(),
        }
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

        let result = config.resolve_profiles_by_path(
            "/home/user/projects",
            |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
        );

        assert!(matches!(
            result.as_deref(),
            Ok([
                ResolvedProfile { entry, .. },
                ResolvedProfile { entry: entry2, .. }
            ]) if entry.name == "home" && entry2.name == "projects"
        ));
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
            matches!(result, Err(ConfigError::NoProfileForDirectory { cwd }) if cwd == "/home/bob/projects")
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
            Ok([ResolvedProfile { entry, .. }]) if entry.name == "standalone"
        ));

        // Test resolving mixed profiles (with and without when_beneath).
        let result = config.resolve_profiles_by_names(
            &["home".to_string(), "standalone".to_string()],
            |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
        );

        assert!(matches!(
            result.as_deref(),
            Ok([
                ResolvedProfile { entry, .. },
                ResolvedProfile { entry: entry2, .. }
            ]) if entry.name == "home" && entry2.name == "standalone"
        ));
    }

    #[test]
    fn test_parse_config_dup_and_sorted() {
        let content = r#"
[[profile]]
name = "b"
when_beneath = "/foo"

[[profile]]
name = "b"
when_beneath = "/foo"

[[profile]]
name = "a"
when_beneath = "/foo"

[[profile]]
name = "b"
when_beneath = "/bar"
"#;
        let config = IslandConfig {
            profiles: IslandConfig::parse_config(content, nocheck_path).unwrap(),
            path: Default::default(),
        };

        let mut profile_iter = config.profiles.iter();

        let profile = profile_iter.next().unwrap();
        assert_eq!(profile.0, "a");

        // Sorted by name and when_beneath.
        let mut entry_iter = profile.1.iter();
        assert_eq!(
            entry_iter.next(),
            Some(&ProfileEntry {
                name: "a".into(),
                when_beneath: Some("/foo".into()),
            })
        );
        assert_eq!(entry_iter.next(), None);

        let profile = profile_iter.next().unwrap();
        assert_eq!(profile.0, "b");

        // Sorted by name and when_beneath.
        let mut entry_iter = profile.1.iter();
        assert_eq!(
            entry_iter.next(),
            Some(&ProfileEntry {
                name: "b".into(),
                when_beneath: Some("/bar".into()),
            })
        );
        assert_eq!(
            entry_iter.next(),
            Some(&ProfileEntry {
                name: "b".into(),
                when_beneath: Some("/foo".into()),
            })
        );
        assert_eq!(entry_iter.next(), None);
        assert_eq!(profile_iter.next(), None);

        // Check duplicate when_beneath with similar name.
        let mut profile_iter = config
            .resolve_profiles_by_path("/foo", |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap()
            .into_iter();
        assert_eq!(
            profile_iter.next().unwrap().entry,
            &ProfileEntry {
                name: "a".into(),
                when_beneath: Some("/foo".into()),
            }
        );
        assert_eq!(
            profile_iter.next().unwrap().entry,
            &ProfileEntry {
                name: "b".into(),
                when_beneath: Some("/foo".into()),
            }
        );
        assert_eq!(profile_iter.next(), None);

        // Check duplicate name with different when_beneath.
        let mut profile_iter = config
            .resolve_profiles_by_names(["b"], |_| -> Result<ResolvedConfig, ConfigError> {
                Ok(create_mock_resolved_config())
            })
            .unwrap()
            .into_iter();
        assert_eq!(
            profile_iter.next().unwrap().entry,
            &ProfileEntry {
                name: "b".into(),
                when_beneath: Some("/bar".into()),
            }
        );
        assert_eq!(
            profile_iter.next().unwrap().entry,
            &ProfileEntry {
                name: "b".into(),
                when_beneath: Some("/foo".into()),
            }
        );
        assert_eq!(profile_iter.next(), None);
    }
}
