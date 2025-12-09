// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// Integration tests runner
//
// This file contains the Rust test runner that executes the shell-based
// integration tests.  It maps Rust test functions to shell script functions.

use std::process::Command;

fn run_test(exe: &str, args: &[&str], error_msg: String) {
    let output = Command::new(exe)
        .args(args)
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
        }
    }
}
