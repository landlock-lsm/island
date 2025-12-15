#!/usr/bin/env zsh
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for the Island Zsh hook integration.
#
# Called by tests/integration.rs

set -ueo pipefail

setopt aliases

HOOK_SCRIPT="$(dirname "$0")/../../assets/shell/hook.zsh"
if [[ ! -f "$HOOK_SCRIPT" ]]; then
    echo "Error: hook.zsh not found at $HOOK_SCRIPT"
    exit 1
fi

# Mock zle command because it's not available in non-interactive scripts.
function zle() {
    :
}

source "$HOOK_SCRIPT"

source "$(dirname "$0")/../tap.sh"

assert_wrapped() {
    local cmd="$1"
    tap_debug "Checking wrapped '$cmd'. List: ${_ISLAND_WRAPPED_CMDS[*]-}"
    if (( ! ${+_ISLAND_WRAPPED_CMDS} )) || (( ! ${_ISLAND_WRAPPED_CMDS[(Ie)$cmd]} )); then
        tap_fail "Command '$cmd' was not wrapped. Wrapped: ${_ISLAND_WRAPPED_CMDS[*]-}"
    fi
    if ! functions "$cmd" >/dev/null; then
        tap_debug "functions $cmd failed"
        functions "$cmd"
        tap_fail "Wrapper function for '$cmd' does not exist"
    fi
}

assert_not_wrapped() {
    local cmd="$1"
    if (( ${+_ISLAND_WRAPPED_CMDS} )) && (( ${_ISLAND_WRAPPED_CMDS[(Ie)$cmd]} )); then
        tap_fail "Command '$cmd' was wrapped but shouldn't be."
    fi
}

cleanup() {
    _island_precmd
    # Clear aliases and functions created during test.
    unalias -m '*' 2>/dev/null || true
}

test_simple_external() {
    tap_start "Simple external command (ls)"
    cleanup
    _island_wrap_cmd "ls"
    assert_wrapped "ls"
    tap_pass
}

test_simple_alias() {
    tap_start "Simple alias (ll -> ls -l)"
    cleanup
    alias ll="ls -l"
    _island_wrap_cmd "ll"
    assert_wrapped "ls"
    assert_not_wrapped "ll"
    tap_pass
}

test_recursive_alias() {
    tap_start "Recursive alias (la -> ll -> ls)"
    cleanup
    alias l="ls"
    alias ll="l -l"
    alias la="ll -a"
    _island_wrap_cmd "la"
    assert_wrapped "ls"
    assert_not_wrapped "l"
    assert_not_wrapped "ll"
    assert_not_wrapped "la"
    tap_pass
}

test_alias_env_var() {
    tap_start "Alias with env var (mygrep -> GREP_COLOR=... grep)"
    cleanup
    alias grep="grep --color=auto"
    alias mygrep="GREP_COLOR='1;32' grep"
    _island_wrap_cmd "mygrep"
    assert_wrapped "grep"
    tap_pass
}

test_alias_precommand() {
    tap_start "Alias with precommand modifier (exec_ls -> exec ls)"
    cleanup
    alias exec_ls="exec ls"
    _island_wrap_cmd "exec_ls"
    assert_wrapped "ls"
    assert_not_wrapped "exec"
    tap_pass
}

test_complex_alias() {
    tap_start "Complex alias (complex -> VAR=1 nocorrect ls)"
    cleanup
    alias complex="VAR1=1 VAR2=2 nocorrect exec ls -la"
    _island_wrap_cmd "complex"
    assert_wrapped "ls"
    assert_not_wrapped "nocorrect"
    tap_pass
    assert_not_wrapped "exec"
}

test_existing_function() {
    tap_start "Existing function"
    cleanup
    function myfunc() { echo "func"; }
    _island_wrap_cmd "myfunc"
    assert_not_wrapped "myfunc"
    unfunction myfunc
    tap_pass
}

test_builtin() {
    tap_start "Builtin (cd)"
    cleanup
    _island_wrap_cmd "cd"
    assert_not_wrapped "cd"
    tap_pass
}

test_nonexistent() {
    tap_start "Non-existent command"
    cleanup
    _island_wrap_cmd "nonexistentcommand"
    assert_not_wrapped "nonexistentcommand"
    tap_pass
}

test_path_command() {
    tap_start "Command with path (/usr/bin/ls)"
    cleanup
    _island_wrap_cmd "/usr/bin/ls"
    assert_not_wrapped "/usr/bin/ls"
    tap_pass
}

test_idempotency() {
    tap_start "Idempotency"
    source "$HOOK_SCRIPT"
    if [[ "$(type -w _island_accept_line)" != "_island_accept_line: function" ]]; then
        tap_fail "_island_accept_line is not a function after re-sourcing"
    fi
    tap_pass
}

test_alias_collision() {
    tap_start "Alias collision (ls is alias, wrap ls)"
    cleanup
    alias ls="ls --color=auto"
    _island_wrap_cmd "ls"
    assert_wrapped "ls"
    tap_pass
}

test_alias_eval() {
    tap_start "Alias with eval (eval_ls -> eval ls)"
    cleanup
    alias eval_ls="eval ls"
    _island_wrap_cmd "eval_ls"
    assert_wrapped "ls"
    tap_pass
}

test_nosandbox() {
    tap_start "nosandbox (nosandbox ls -> ls)"
    cleanup
    _island_accept_line_wrapper() {
        BUFFER="nosandbox ls"
        _island_accept_line
    }

    # Mock zle to capture the buffer modification.
    zle() {
        if [[ "$1" == "_island_orig_accept_line" ]]; then
            tap_debug "Final BUFFER: $BUFFER"
        fi
    }

    # We need to mock _island_wrap_cmd to verify it's NOT called for ls.
    _island_wrap_cmd() {
        tap_debug "_island_wrap_cmd called for $1"
        _ISLAND_WRAPPED_CMDS+=("$1")
    }

    BUFFER="nosandbox ls"
    _island_accept_line
    if (( ${+_ISLAND_WRAPPED_CMDS} )) && (( ${_ISLAND_WRAPPED_CMDS[(Ie)ls]} )); then
        tap_fail "nosandbox ls resulted in wrapping ls"
    fi

    # nosandbox is an alias to empty string, so it stays in the buffer but
    # effectively disappears during execution.
    if [[ "$BUFFER" != "nosandbox ls" ]]; then
        tap_fail "Buffer was unexpectedly modified. Buffer: '$BUFFER'"
    fi
    tap_pass
}

test_precmd_cleanup() {
    tap_start "Cleanup of wrapped commands"
    cleanup

    # Simulate a state where precmd was skipped.
    _ISLAND_PROFILES=("default")
    function old_cmd() { echo "old"; }
    _ISLAND_WRAPPED_CMDS=("old_cmd")

    # Set BUFFER to a new command to trigger accept_line logic.
    BUFFER="ls"

    _island_accept_line

    if functions old_cmd >/dev/null; then
        tap_fail "old_cmd wrapper was not cleaned up"
    fi

    assert_wrapped "ls"

    tap_pass
}

TESTS=(
    test_simple_external
    test_simple_alias
    test_recursive_alias
    test_alias_env_var
    test_alias_precommand
    test_complex_alias
    test_existing_function
    test_builtin
    test_nonexistent
    test_path_command
    test_idempotency
    test_alias_collision
    test_alias_eval
    test_nosandbox
    test_precmd_cleanup
)

tap_run "$@"
