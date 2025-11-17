// SPDX-License-Identifier: Apache-2.0 OR MIT

#![cfg(test)]

use crate::{
    config::{tests::create_resolved_profile, ConfigError, IslandConfig},
    context::ContextEntry,
    Verbose,
};
use landlockconfig::ResolvedConfig;
use std::{collections::BTreeMap, env, io};

fn get_test_home(user: &str) -> String {
    let project_root = env::var("CARGO_MANIFEST_DIR").unwrap();
    format!("{}/tests/home/{}", project_root, user)
}

fn get_test_env(user: &str) -> BTreeMap<String, String> {
    let test_home = get_test_home(user);
    [
        ("XDG_CONFIG_HOME", format!("{}/config", test_home)),
        ("XDG_DATA_HOME", format!("{}/share", test_home)),
        ("XDG_STATE_HOME", format!("{}/state", test_home)),
        ("XDG_CACHE_HOME", format!("{}/cache", test_home)),
        ("XDG_RUNTIME_DIR", format!("{}/run", test_home)),
        ("TMPDIR", format!("{}/tmp", test_home)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

fn get_read_env(user: &str) -> impl Fn(&str) -> Result<String, env::VarError> {
    let env_vars = get_test_env(user);
    move |name: &str| -> Result<String, env::VarError> {
        env_vars
            .get(name)
            .map(|v| Ok(v.to_string()))
            .unwrap_or(Err(env::VarError::NotPresent))
    }
}

#[test]
fn test_profile_missing() {
    let read_env = get_read_env("does-not-exist");
    let expected_path = format!("{}/config/island/profiles", get_test_home("does-not-exist"));
    let config = IslandConfig::new(&read_env);

    assert!(matches!(
        config,
        Err(ConfigError::ProfilesDirectory { path, source })
            if path == expected_path && source.kind() == io::ErrorKind::NotFound
    ));
}

#[test]
fn test_profile_mini() {
    let read_env = get_read_env("mini-landlock-config");
    let config = IslandConfig::new(&read_env).unwrap();
    let load_config = |name: &str| -> Result<ResolvedConfig, ConfigError> {
        config.load_landlock_config(name).map_err(|e| e.into())
    };

    let resolved_profiles = config
        .resolve_profiles_by_names(&["foo", "foo"], load_config)
        .unwrap();
    let mut profiles_iter = resolved_profiles.iter();
    assert!(matches!(profiles_iter.next(), Some(profile)
            if profile.name == "foo" && profile.context.is_none()));
    // TODO: Deduplicate profiles while keeping the order.
    assert!(matches!(profiles_iter.next(), Some(profile)
            if profile.name == "foo" && profile.context.is_none()));
    assert_eq!(profiles_iter.next(), None);

    let resolved_profiles = config.resolve_profiles_by_names(&["bar"], load_config);
    assert!(matches!(
        resolved_profiles,
        Err(ConfigError::ProfileNotFound {
            name
        }) if name == "bar"
    ));

    let resolved_profiles = config
        .resolve_profiles_by_path("/tmp/foo/bar", load_config)
        .unwrap();
    let mut profiles_iter = resolved_profiles.iter();
    assert!(matches!(profiles_iter.next(), Some(profile)
        if profile.name == "foo" &&
            profile.context == Some(
                &ContextEntry { when_beneath: Some("/tmp".into())}
            )
    ));
    assert_eq!(profiles_iter.next(), None);

    assert!(matches!(
        config.resolve_profiles_by_path("/", load_config),
        Err(ConfigError::NoContextForDirectory { cwd }) if cwd == "/"
    ));
}

#[test]
fn test_workspace_manager_empty() {
    let verbose = Verbose(true);
    let read_env = get_read_env("no-landlock-config");
    let config = IslandConfig::new(&read_env).unwrap();

    let resolved_profile = create_resolved_profile("foo", None);
    assert!(resolved_profile.workspace);

    let manager = resolved_profile
        .workspace_manager(&config, &verbose, &read_env)
        .unwrap();
    assert!(!manager.env_vars.is_empty());

    let mut resolved_profile = create_resolved_profile("bar", None);
    resolved_profile.workspace = false;
    let manager = resolved_profile
        .workspace_manager(&config, &verbose, &read_env)
        .unwrap();
    assert!(manager.env_vars.is_empty());
}
