#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for Island version.
#
# This guarantees that we are testing the correct build.
#
# Called by tests/integration.rs

set -ueo pipefail

source "$(dirname "$0")/../tap.sh"

test_version() {
    local output commit_hash

    tap_start "island --version"

    output=$(island --version)

    # Get the current git commit hash, ignoring the "-dirty" suffix if present.
    commit_hash=$(git rev-parse --short=12 HEAD)

    if [[ "$output" != *"$commit_hash"* ]]; then
        tap_fail "island --version output does not contain commit hash '$commit_hash'. Output: '$output'"
    fi

    tap_pass
}

TESTS=(
    test_version
)

tap_run "$@"
