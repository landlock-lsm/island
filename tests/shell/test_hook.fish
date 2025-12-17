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

set -gx HOME (mktemp -d)
set -l SCRIPT_DIR (dirname (status filename))
set -l HOOK_SCRIPT "$SCRIPT_DIR/../../assets/shell/hook.fish"

set -x PATH "$SCRIPT_DIR/test_stub:$PATH"

set -g __island_cmdline_buffer ""
set -g __island_cmdline_valid_status 0
set -g __island_cmdline_paging_mode 0

function commandline
    set -l cmd $argv[1]
    switch $cmd
        case "--is-valid"
            return $__island_cmdline_valid_status
        case "--paging-mode"
            if test "$__island_cmdline_paging_mode" -eq 1
                return 0
            else
                return 1
            end
        case "--current-buffer"
            printf "%s" "$__island_cmdline_buffer"
            return 0
        case "--replace"
            set -l idx 2
            if test (count $argv) -ge 2 -a "$argv[2]" = "--"
                set idx 3
            end
            if test (count $argv) -ge $idx
                set -l rest $argv[$idx..-1]
                set -g __island_cmdline_buffer "$rest"
            else
                set -g __island_cmdline_buffer ""
            end
            return 0
    end
    return 0
end

function bind
    :
end

source "$HOOK_SCRIPT"

function setup
    # Reset state between tests
    set -g __island_cmdline_buffer ""
    set -g __island_cmdline_valid_status 0
    set -g __island_cmdline_paging_mode 0
    set -gx ISLAND_STATUS_OUTPUT ""
    set -gx ISLAND_STATUS_EXIT 0
    set -gx ISLAND_RUN_LOG (mktemp)
    set -e _ISLAND_PROFILES
    set -e _ISLAND_WRAPPED_CMDS
end

function assert_eq --argument-names actual expected msg
    if test "$actual" != "$expected"
        tap_fail "$msg (expected '$expected', got '$actual')"
    end
end

function assert_contains --argument-names needle msg
    set -l list $argv[3..-1]
    if not contains -- $needle $list
        tap_fail "$msg"
    end
end

function assert_not_contains --argument-names needle msg
    set -l list $argv[3..-1]
    if contains -- $needle $list
        tap_fail "$msg"
    end
end

function test_profiles_tracking
    tap_start "Profiles tracking via _island_chpwd"
    setup
    set -gx ISLAND_STATUS_OUTPUT a b
    _island_chpwd
    assert_eq "$_ISLAND_PROFILES" "a b" "_ISLAND_PROFILES not set"

    set -gx ISLAND_STATUS_EXIT 1
    _island_chpwd
    if set -q _ISLAND_PROFILES[1]
        tap_fail "_ISLAND_PROFILES not cleared on failure"
    end
    tap_pass
end

function test_path_rewrite
    tap_start "Path command rewrites buffer"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "/bin/echo hi"
    _island_accept_line
    assert_eq "$__island_cmdline_buffer" "island run -- /bin/echo hi" "Buffer not rewritten"
    tap_pass
end

function test_path_rewrite_quoted
    tap_start "Path rewrite preserves quoting"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "./'my binary' arg"
    _island_accept_line
    assert_eq "$__island_cmdline_buffer" "island run -- ./'my binary' arg" "Quoted path not rewritten"
    tap_pass
end

function test_path_rewrite_escaped
    tap_start "Path rewrite handles escaped spaces"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "./my\\ binary"
    _island_accept_line
    assert_eq "$__island_cmdline_buffer" "island run -- ./my\\ binary" "Escaped path not rewritten"
    tap_pass
end

function test_path_rewrite_space
    tap_start "Path rewrite handles space-quoted path"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "./my binary"
    _island_accept_line
    assert_eq "$__island_cmdline_buffer" "island run -- ./my binary" "Space path not rewritten"
    tap_pass
end

function test_quoted_command_wrapping
    tap_start "Quoted args keep command wrapping"
    setup
    set -g _ISLAND_PROFILES alpha
    functions -e ls 2>/dev/null
    set -g __island_cmdline_buffer "ls \"file with >> weird name\""
    _island_accept_line
    assert_contains ls "ls not wrapped" $_ISLAND_WRAPPED_CMDS
    assert_eq "$__island_cmdline_buffer" "ls \"file with >> weird name\"" "Buffer modified unexpectedly"
    tap_pass
end

function test_nosandbox
    tap_start "nosandbox bypasses wrapping"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "nosandbox /bin/echo hi"
    _island_accept_line
    assert_eq "$__island_cmdline_buffer" "nosandbox /bin/echo hi" "Buffer should remain unchanged"
    assert_not_contains /bin/echo "nosandbox should skip wrapping" $_ISLAND_WRAPPED_CMDS
    tap_pass
end

function test_and_variants
    tap_start "Logical and separators"
    setup
    set -g _ISLAND_PROFILES alpha

    set -g __island_cmdline_buffer "head a; and tail b"
    _island_accept_line
    assert_contains head "first command not wrapped" $_ISLAND_WRAPPED_CMDS
    assert_contains tail "command not wrapped after \`; and\`" $_ISLAND_WRAPPED_CMDS

    set -g __island_cmdline_buffer "head a and tail b"
    _island_accept_line
    assert_contains head "first command not wrapped" $_ISLAND_WRAPPED_CMDS
    if contains -- tail $_ISLAND_WRAPPED_CMDS
        tap_fail "'and' without leading separator should not wrap second command"
    end
    tap_pass
end

function test_pipe_wrapping
    tap_start "Wrapping with pipe variants"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "echo hi &| cat 2>| head"
    _island_accept_line
    tap_debug "Wrapped: $_ISLAND_WRAPPED_CMDS"
    assert_contains cat "cat not wrapped" $_ISLAND_WRAPPED_CMDS
    assert_contains head "head not wrapped" $_ISLAND_WRAPPED_CMDS
    tap_pass
end

function test_redirections
    tap_start "Redirections split tokens without new command"
    setup
    set -g _ISLAND_PROFILES alpha
    functions -e ls 2>/dev/null

    set -g __island_cmdline_buffer "ls>a"
    _island_accept_line
    assert_contains ls "ls not wrapped for >" $_ISLAND_WRAPPED_CMDS
    assert_eq "$__island_cmdline_buffer" "ls>a" "Buffer changed with >"

    set -g __island_cmdline_buffer "ls>>a"
    _island_accept_line
    assert_contains ls "ls not wrapped for >>" $_ISLAND_WRAPPED_CMDS
    assert_eq "$__island_cmdline_buffer" "ls>>a" "Buffer changed with >>"

    set -g __island_cmdline_buffer "ls<a"
    _island_accept_line
    assert_contains ls "ls not wrapped for <" $_ISLAND_WRAPPED_CMDS
    assert_eq "$__island_cmdline_buffer" "ls<a" "Buffer changed with <"

    set -g __island_cmdline_buffer "ls<<a"
    _island_accept_line
    assert_contains ls "ls not wrapped for <<" $_ISLAND_WRAPPED_CMDS
    assert_eq "$__island_cmdline_buffer" "ls<<a" "Buffer changed with <<"

    tap_pass
end

function test_invalid_commandline
    tap_start "Invalid commandline skips wrapping"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "echo 'unterminated"
    set -g __island_cmdline_valid_status 2
    _island_accept_line
    assert_eq "$__island_cmdline_buffer" "echo 'unterminated" "Buffer modified unexpectedly"
    if set -q _ISLAND_WRAPPED_CMDS[1]
        tap_fail "Wrappers created on invalid buffer"
    end
    tap_pass
end

function test_cleanup_event
    tap_start "Cleanup on fish_prompt event"
    setup
    set -g _ISLAND_PROFILES alpha
    _island_wrap_cmd cat
    if not functions -q cat
        tap_fail "Wrapper function missing before cleanup"
    end
    emit fish_prompt
    if set -q _ISLAND_WRAPPED_CMDS[1]
        tap_fail "_ISLAND_WRAPPED_CMDS not cleared"
    end
    if functions -q cat
        tap_fail "Wrapper function not removed"
    end
    emit fish_prompt
    tap_pass
end

function test_island_refreshes_profiles
    tap_start "island function refreshes profiles"
    setup
    set -gx ISLAND_STATUS_OUTPUT "first"
    _island_chpwd
    assert_eq "$_ISLAND_PROFILES" "first" "Initial profiles mismatch"

    set -gx ISLAND_STATUS_OUTPUT "second"
    island status >/dev/null 2>&1
    assert_eq "$_ISLAND_PROFILES" "second" "Profiles not refreshed"
    tap_pass
end

function test_paging_mode_skip
    tap_start "Paging mode skips hook processing"
    setup
    set -g _ISLAND_PROFILES alpha
    set -g __island_cmdline_buffer "/bin/echo hi"
    set -g __island_cmdline_paging_mode 1  # Paging mode active (returns 0 = true)
    _island_accept_line
    # When in paging mode, the buffer should not be modified
    assert_eq "$__island_cmdline_buffer" "/bin/echo hi" "Buffer modified during paging mode"
    if set -q _ISLAND_WRAPPED_CMDS[1]
        tap_fail "Commands wrapped during paging mode"
    end
    tap_pass
end

set TESTS \
    test_profiles_tracking \
    test_path_rewrite \
    test_path_rewrite_quoted \
    test_path_rewrite_escaped \
    test_path_rewrite_space \
    test_quoted_command_wrapping \
    test_nosandbox \
    test_and_variants \
    test_pipe_wrapping \
    test_redirections \
    test_invalid_commandline \
    test_cleanup_event \
    test_island_refreshes_profiles \
    test_paging_mode_skip

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
