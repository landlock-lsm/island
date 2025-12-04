// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::process::Command;

fn main() {
    let commit = match Command::new("git").args(["rev-parse", "HEAD"]).output() {
        Ok(output) if output.status.success() => {
            // Do not rely on local configuration (i.e. core.abbrev) for length.
            String::from_utf8_lossy(&output.stdout)
                .chars()
                .take(12)
                .collect()
        }
        _ => "unknown".to_string(),
    };

    println!("cargo:rustc-env=GIT_COMMIT={}", commit);
    println!("cargo:rerun-if-changed=.git/HEAD");
}
