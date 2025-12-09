#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for the Island create and related commands.
#
# Called by tests/integration.rs

set -ueo pipefail

source "$(dirname "$0")/../tap.sh"

test_create_cwd() {
    local output

    tap_start "island create (current directory)"

    tap_setup

    if island status; then
        tap_fail "island status should fail when no profile"
    fi

    island create bar

    if [[ ! -f "$XDG_CONFIG_HOME/island/profiles/bar/profile.toml" ]]; then
        tap_fail "Profile file not created at $XDG_CONFIG_HOME/island/profiles/bar/profile.toml"
    fi

    # Should succeed and output profile name.
    output=$(island status)

    if [[ "$output" != "bar" ]]; then
        tap_fail "island status output mismatch. Expected 'bar', got '$output'"
    fi

    tap_pass
}

test_create_with_directories() {
    local output

    tap_start "island create (with directory names)"

    tap_setup

    mkdir subdir

    if island status; then
        tap_fail "island status should fail when no profile"
    fi

    island create foo -b subdir

    if [[ ! -f "$XDG_CONFIG_HOME/island/profiles/foo/profile.toml" ]]; then
        tap_fail "Profile file not created at $XDG_CONFIG_HOME/island/profiles/foo/profile.toml"
    fi

    if island status; then
        tap_fail "island status should fail in non-profile directory"
    fi

    cd subdir
    # Should succeed and output profile name.
    output=$(island status)

    if [[ "$output" != "foo" ]]; then
        tap_fail "island status output mismatch. Expected 'foo', got '$output'"
    fi

    island create bar -b ..

    if [[ ! -f "$XDG_CONFIG_HOME/island/profiles/bar/profile.toml" ]]; then
        tap_fail "Profile file not created at $XDG_CONFIG_HOME/island/profiles/bar/profile.toml"
    fi

    # Should succeed and output profile names (separated by newlines).
    output=$(island status | tr '\n' '!')

    if [[ "$output" != "bar!foo!" ]]; then
        tap_fail "island status output mismatch. Expected 'bar foo', got '$output'"
    fi

    tap_pass
}

TESTS=(
    test_create_cwd
    test_create_with_directories
)

tap_run "$@"
