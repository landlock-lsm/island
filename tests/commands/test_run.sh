#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for the Island run command.
#
# Called by tests/integration.rs

set -ueo pipefail

source "$(dirname "$0")/../tap.sh"

test_run_implicit_profile() {
    local output ret

    tap_start "island run (implicit profile)"
    tap_setup

    island create foo

    output=$(island run -- echo "hello")

    if [[ "$output" != "hello" ]]; then
        tap_fail "Expected 'hello', got '$output'"
    fi

    mkdir subdir
    cd subdir

    cd ../..

    ret=0
    island run -- true || ret=$?

    if [[ "$ret" -ne 1 ]]; then
        tap_fail "Expected exit code 1, got $ret"
    fi

    tap_pass
}

test_run_explicit_profiles() {
    local ret output

    tap_start "island run -p <profile1> -p <profile2>"
    tap_setup

    island create foo

    output=$(island run -p foo -- echo "hello")

    if [[ "$output" != "hello" ]]; then
        tap_fail "Expected 'hello', got '$output'"
    fi

    ret=0
    island run -p foo -p bar -- true || ret=$?

    if [[ "$ret" -ne 1 ]]; then
        tap_fail "Expected exit code 1, got $ret"
    fi

    mkdir subdir
    island create -b subdir bar

    island run -p foo -p bar -- true

    tap_pass
}

test_run_exit_code() {
    local ret

    tap_start "island run exit code propagation"
    tap_setup

    island create bar

    # Should fail with exit code 1.
    if island run -p bar -- false; then
        tap_fail "island run should fail when command fails"
    fi

    # Check specific exit code.
    ret=0
    island run -p bar -- bash -c "exit 2" || ret=$?

    if [[ "$ret" -ne 2 ]]; then
        tap_fail "Expected exit code 2, got $ret"
    fi

    tap_pass
}

test_run_restrict_access_dir() {
    local output ret

    tap_start "island run with directory access restrictions"
    tap_setup

    island create foo

    island run -- ls .

    ret=0
    output="$(island run -- ls .. 2>&1)" || ret=$?

    if [[ "$ret" -ne 2 ]]; then
        tap_fail "Expected exit code 2, got $ret"
    fi

    if [[ "$output" != "ls: cannot open directory '..': Permission denied" ]]; then
        tap_fail "Expected specific error message, got '$output'"
    fi

    tap_pass
}

test_run_restrict_signal() {
    local output ret pid

    tap_start "island run with signal restrictions (Landlock ABI >= 6)"
    tap_setup

    island create foo

    # Try to send a signal to the parent process (outside the sandbox).
    pid=$$
    ret=0
    output="$(island run -- kill -HUP "$pid" 2>&1)" || ret=$?

    if [[ "$ret" -ne 1 ]]; then
        tap_fail "Expected exit code 1, got $ret"
    fi

    if [[ "$output" != "kill: sending signal to $pid failed: Operation not permitted" ]]; then
        tap_fail "Expected specific error message, got '$output'"
    fi

    tap_pass
}

TESTS=(
    test_run_implicit_profile
    test_run_explicit_profiles
    test_run_exit_code
    test_run_restrict_access_dir
    test_run_restrict_signal
)

tap_run "$@"
