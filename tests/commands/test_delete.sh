#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for the Island delete command.
#
# Called by tests/integration.rs

set -ueo pipefail

source "$(dirname "$0")/../tap.sh"

test_delete_existing() {
    tap_start "island delete (existing profile)"
    tap_setup

    island create foo

    local profile_dir="$XDG_CONFIG_HOME/island/profiles/foo"

    if [[ ! -d "$profile_dir" ]]; then
        tap_fail "Profile directory not created"
    fi

    island delete foo

    if [[ -d "$profile_dir" ]]; then
        tap_fail "Profile directory was not deleted"
    fi

    tap_pass
}

test_delete_nonexistent() {
    tap_start "island delete (nonexistent profile)"
    tap_setup

    if island delete nonexistent; then
        tap_fail "island delete should fail for nonexistent profile"
    fi

    tap_pass
}

test_delete_invalid_name() {
    tap_start "island delete (invalid profile name)"
    tap_setup

    if island delete "-foo"; then
        tap_fail "island delete should fail for invalid profile name"
    fi

    tap_pass
}

TESTS=(
    test_delete_existing
    test_delete_nonexistent
    test_delete_invalid_name
)

tap_run "$@"
