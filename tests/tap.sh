#!/usr/bin/env sh
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# This file should be compatible with both Bash and Zsh.

TEST_NUM=0
CURRENT_TEST_DESC=""

tap_debug() {
    echo "# DEBUG: $*" >&2
}

tap_start() {
    TEST_NUM=$((TEST_NUM + 1))
    CURRENT_TEST_DESC="$1"
}

tap_fail() {
    echo "not ok $TEST_NUM - $CURRENT_TEST_DESC: $1"
    exit 1
}

tap_pass() {
    echo "ok $TEST_NUM - $CURRENT_TEST_DESC"
}

tap_plan() {
    echo "1..$1"
}

tap_setup() {
    ISLAND_TMPDIR="$(mktemp --tmpdir='' --directory island-test.XXXXXXXXXX)"
    export ISLAND_TMPDIR
    trap tap_teardown QUIT INT TERM EXIT

    export XDG_CONFIG_HOME="$ISLAND_TMPDIR/config"
    mkdir -p "$XDG_CONFIG_HOME"

    cd "$ISLAND_TMPDIR"
    # The current working directory will be reset for each test thanks to the
    # subshell in tap_run.
}

tap_teardown() {
    if [[ -n "${ISLAND_TMPDIR:-}" ]]; then
        if [[ -d "$ISLAND_TMPDIR" ]]; then
            rm -rf "$ISLAND_TMPDIR"
        fi
        unset ISLAND_TMPDIR
    fi
    trap - QUIT INT TERM EXIT
}

tap_run() {
    # Ensure island is in PATH
    if ! command -v island >/dev/null; then
        echo "Error: island not found in PATH"
        exit 1
    fi

    if [[ $# -gt 0 ]]; then
        if [[ "$1" == "--check-count" ]]; then
            local expected="$2"
            local actual="${#TESTS[@]}"
            if [[ "$actual" -ne "$expected" ]]; then
                echo "Error: Expected $expected tests, but found $actual."
                exit 1
            fi
            exit 0
        fi

        local test_name="$1"
        tap_debug "Running test: $test_name"
        (
            "$test_name"
        )
        tap_teardown
    else
        tap_plan ${#TESTS}
        for t in "${TESTS[@]}"; do
            (
                "$t"
            )
            tap_teardown
        done
    fi
}
