#!/usr/bin/env fish
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -g TEST_NUM 0
set -g CURRENT_TEST_DESC ""

function tap_start --argument-names desc
    set -g TEST_NUM (math $TEST_NUM + 1)
    set -g CURRENT_TEST_DESC $desc
end

function tap_debug --argument-names msg
    echo "# DEBUG: $msg" >&2
end

function tap_fail --argument-names msg
    echo "not ok $TEST_NUM - $CURRENT_TEST_DESC: $msg"
    exit 1
end

function tap_pass
    echo "ok $TEST_NUM - $CURRENT_TEST_DESC"
end

function tap_plan --argument-names count
    echo "1..$count"
end

function tap_run
    if test (count $argv) -gt 0
        if test "$argv[1]" = "--check-count"
            set -l expected $argv[2]
            if test (count $TESTS) -ne $expected
                echo "Error: Expected $expected tests, but found "(count $TESTS)"."
                exit 1
            end
            exit 0
        end

        set -l test_name $argv[1]
        if not contains -- $test_name $TESTS
            tap_fail "Unknown test $test_name"
        end
        $test_name
        exit 0
    end

    tap_plan (count $TESTS)
    for t in $TESTS
        eval $t
    end
end
