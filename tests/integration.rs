// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// Integration tests runner
//
// This file contains the Rust test runner that executes the shell-based
// integration tests.  It maps Rust test functions to shell script functions.

use std::process::Command;

fn get_envs() -> Vec<(&'static str, String)> {
    let bin_path = env!("CARGO_BIN_EXE_island");
    let bin_dir = std::path::Path::new(bin_path).parent().unwrap();
    let path = std::env::var("PATH").unwrap();
    let new_path = format!("{}:{}", bin_dir.display(), path);
    vec![("PATH", new_path)]
}

fn run_test(exe: &str, args: &[&str], error_msg: String) {
    let output = Command::new(exe)
        .args(args)
        .envs(get_envs())
        .output()
        .expect("failed to execute test program");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Print output to help debugging if the test fails.
    println!("{}", stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "{} with exit code: {:?}\nStderr:\n{}",
            error_msg,
            output.status.code(),
            stderr
        );
    }
}

macro_rules! run_tests {
    ($exe:expr, { $($test_name:ident),* $(,)? }) => {
        #[test]
        fn _check_test_count() {
            let count = [ $(stringify!($test_name)),* ].len();
            super::run_test(
                $exe,
                &["--check-count", &count.to_string()],
                format!("Test count check failed for {}. Expected: {}", $exe, count)
            );
        }

        $(
            #[test]
            fn $test_name() {
                let test_func = stringify!($test_name);
                super::run_test(
                    $exe,
                    &[test_func],
                    format!("Test {} failed", test_func)
                );
            }
        )*
    }
}

mod version {
    run_tests! {
        "tests/commands/test_version.sh",
        {
            test_version,
        }
    }
}

mod shell_hook {
    run_tests! {
        "tests/shell/test_hook.zsh",
        {
            test_simple_external,
            test_simple_alias,
            test_recursive_alias,
            test_alias_env_var,
            test_alias_precommand,
            test_complex_alias,
            test_existing_function,
            test_builtin,
            test_nonexistent,
            test_path_command,
            test_idempotency,
            test_alias_collision,
            test_alias_eval,
            test_nosandbox,
            test_precmd_cleanup,
        }
    }
}

mod shell_hook_fish {
    run_tests! {
        "tests/shell/test_hook.fish",
        {
            test_profiles_tracking,
            test_path_rewrite,
            test_path_rewrite_quoted,
            test_path_rewrite_escaped,
            test_path_rewrite_space,
            test_quoted_command_wrapping,
            test_nosandbox,
            test_operators,
            test_and_variants,
            test_pipe_wrapping,
            test_redirections,
            test_invalid_commandline,
            test_cleanup_event,
            test_island_refreshes_profiles,
            test_paging_mode_skip,
        }
    }
}

mod create {
    run_tests! {
        "tests/commands/test_create.sh",
        {
            test_create_cwd,
            test_create_with_directories,
        }
    }
}

mod run {
    run_tests! {
        "tests/commands/test_run.sh",
        {
            test_run_implicit_profile,
            test_run_explicit_profiles,
            test_run_exit_code,
            test_run_restrict_access_dir,
            test_run_restrict_signal,
        }
    }
}

mod update {
    run_tests! {
        "tests/commands/test_update.sh",
        {
            test_update_explicit,
            test_update_implicit,
            test_update_all,
            test_update_nonexistent,
        }
    }
}
