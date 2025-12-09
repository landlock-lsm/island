#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for the Island update command.
#
# Called by tests/integration.rs

set -ueo pipefail

source "$(dirname "$0")/../tap.sh"

test_update_implicit() {
    tap_start "island update (implicit profile)"
    tap_setup

    mkdir project
    island create foo -b project

    local profile_dir="$XDG_CONFIG_HOME/island/profiles/foo/landlock"
    local default_file="$profile_dir/island-default-base.toml"

    echo "# modified" >> "$default_file"

    cd project
    island update

    if grep -q "# modified" "$default_file"; then
        tap_fail "Default file was not updated/restored"
    fi

    tap_pass
}

test_update_explicit() {
    tap_start "island update (explicit profile)"
    tap_setup

    island create foo
    island create bar

    local foo_default="$XDG_CONFIG_HOME/island/profiles/foo/landlock/island-default-base.toml"
    local bar_default="$XDG_CONFIG_HOME/island/profiles/bar/landlock/island-default-base.toml"

    echo "# modified" >> "$foo_default"
    echo "# modified" >> "$bar_default"

    if ! grep -q "# modified" "$foo_default"; then
        tap_fail "Failed to modify default file for foo"
    fi

    if ! grep -q "# modified" "$bar_default"; then
        tap_fail "Failed to modify default file for bar"
    fi

    island update -p foo

    if grep -q "# modified" "$foo_default"; then
        tap_fail "Foo default file was not updated"
    fi

    if ! grep -q "# modified" "$bar_default"; then
        tap_fail "Bar default file was wrongly updated"
    fi

    tap_pass
}

test_update_all() {
    tap_start "island update --all"
    tap_setup

    mkdir subdir
    island create foo -b subdir
    island create bar -b subdir

    local foo_default="$XDG_CONFIG_HOME/island/profiles/foo/landlock/island-default-base.toml"
    local bar_default="$XDG_CONFIG_HOME/island/profiles/bar/landlock/island-default-base.toml"

    echo "# modified" >> "$foo_default"
    echo "# modified" >> "$bar_default"

    if ! grep -q "# modified" "$foo_default"; then
        tap_fail "Failed to modify default file for foo"
    fi

    if ! grep -q "# modified" "$bar_default"; then
        tap_fail "Failed to modify default file for bar"
    fi

    island update --all

    if grep -q "# modified" "$foo_default"; then
        tap_fail "Foo default file was not updated"
    fi

    if grep -q "# modified" "$bar_default"; then
        tap_fail "Bar default file was not updated"
    fi

    tap_pass
}

test_update_nonexistent() {
    tap_start "island update (nonexistent profile)"
    tap_setup

    if island update -p nonexistent; then
        tap_fail "island update should fail for nonexistent profile"
    fi

    tap_pass
}

TESTS=(
    test_update_explicit
    test_update_implicit
    test_update_all
    test_update_nonexistent
)

tap_run "$@"
