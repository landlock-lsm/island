// SPDX-License-Identifier: Apache-2.0 OR MIT

use landlockconfig::{Config, ConfigFormat, ParseDirectoryError, ResolveError, ResolvedConfig};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
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

#[derive(Debug)]
pub struct ResolvedProfile {
    pub name: String,
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

#[derive(Debug, Deserialize, Clone)]
pub struct ProfileEntry {
    // TODO: Restrict name.
    pub name: String,
    pub when_beneath: Option<PathBuf>,
}

type Profiles = BTreeMap<String, ProfileEntry>;

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
            profiles: Self::parse_config(&fs::read_to_string(path.join("main.toml"))?)?,
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

    fn parse_config(content: &str) -> Result<Profiles, ConfigError> {
        let main_config: TomlConfig = toml::from_str(content)?;

        let mut profiles = BTreeMap::new();
        for mut profile in main_config.profiles {
            // Canonicalize the when_beneath path to resolve symlinks and ignore
            // profiles with non-existing directories.
            if let Some(when_beneath) = &profile.when_beneath {
                if let Ok(canonical_path) = when_beneath.canonicalize() {
                    profile.when_beneath = Some(canonical_path);
                    profiles.insert(profile.name.clone(), profile);
                } else {
                    eprintln!(
                        "Warning: ignoring profile \"{}\" because of non-existing directory \"{}\"",
                        profile.name,
                        when_beneath.display()
                    );
                }
            } else {
                profiles.insert(profile.name.clone(), profile);
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
    ) -> Result<Vec<ResolvedProfile>, ConfigError>
    where
        P: AsRef<Path>,
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        let canonicalized_path = canonicalized_path.as_ref();

        // Consistently store sorted paths for scope ordering and determinism.
        let matching_profiles: BTreeMap<&PathBuf, &ProfileEntry> = self
            .profiles
            .values()
            .filter_map(|profile| {
                profile.when_beneath.as_ref().and_then(|when_beneath| {
                    if canonicalized_path.starts_with(when_beneath) {
                        Some((when_beneath, profile))
                    } else {
                        None
                    }
                })
            })
            .collect();

        if matching_profiles.is_empty() {
            return Err(ConfigError::NoProfileForDirectory {
                cwd: canonicalized_path.display().to_string(),
            });
        }

        let mut resolved = Vec::new();
        for profile in matching_profiles.into_values() {
            resolved.push(ResolvedProfile {
                name: profile.name.clone(),
                config: load_config(&profile.name).map_err(|e| e.into())?,
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
    pub fn resolve_profiles_by_names<F, E>(
        &self,
        profile_names: &[String],
        load_config: F,
    ) -> Result<Vec<ResolvedProfile>, ConfigError>
    where
        F: Fn(&str) -> Result<ResolvedConfig, E>,
        E: Into<ConfigError>,
    {
        let mut resolved = Vec::new();

        for profile_name in profile_names {
            self.profiles
                .get(profile_name)
                .ok_or_else(|| ConfigError::ProfileNotFound {
                    name: profile_name.to_string(),
                })?;

            resolved.push(ResolvedProfile {
                name: profile_name.to_string(),
                config: load_config(profile_name).map_err(|e| e.into())?,
            });
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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

        // Parse TOML directly without canonicalization for tests.
        let main_config: TomlConfig = toml::from_str(content).unwrap();
        let mut profiles = BTreeMap::new();
        for profile in main_config.profiles {
            profiles.insert(profile.name.clone(), profile);
        }
        IslandConfig {
            profiles,
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
                ResolvedProfile { name, .. },
                ResolvedProfile { name: name2, .. }
            ]) if name == "home" && name2 == "projects"
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
            Ok([ResolvedProfile { name, .. }]) if name == "standalone"
        ));

        // Test resolving mixed profiles (with and without when_beneath).
        let result = config.resolve_profiles_by_names(
            &["home".to_string(), "standalone".to_string()],
            |_| -> Result<ResolvedConfig, ConfigError> { Ok(create_mock_resolved_config()) },
        );

        assert!(matches!(
            result.as_deref(),
            Ok([
                ResolvedProfile { name, .. },
                ResolvedProfile { name: name2, .. }
            ]) if name == "home" && name2 == "standalone"
        ));
    }
}
