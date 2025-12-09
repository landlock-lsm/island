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

tap_run() {
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
        "$test_name"
    else
        tap_plan ${#TESTS}
        for t in "${TESTS[@]}"; do
            $t
        done
    fi
}
