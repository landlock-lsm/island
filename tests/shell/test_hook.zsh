#!/usr/bin/env zsh
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Test script for Island Zsh hook integration.
#
# Run this script with zsh: zsh tests/shell/test_hook.zsh

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

fail() {
    echo "FAIL: $1"
    exit 1
}

assert_wrapped() {
    local cmd="$1"
    echo "DEBUG: Checking wrapped '$cmd'. List: ${_ISLAND_WRAPPED_CMDS[*]-}"
    if (( ! ${+_ISLAND_WRAPPED_CMDS} )) || (( ! ${_ISLAND_WRAPPED_CMDS[(Ie)$cmd]} )); then
        fail "Command '$cmd' was not wrapped. Wrapped: ${_ISLAND_WRAPPED_CMDS[*]-}"
    fi
    if ! functions "$cmd" >/dev/null; then
        echo "DEBUG: functions $cmd failed"
        functions "$cmd"
        fail "Wrapper function for '$cmd' does not exist"
    fi
}

assert_not_wrapped() {
    local cmd="$1"
    if (( ${+_ISLAND_WRAPPED_CMDS} )) && (( ${_ISLAND_WRAPPED_CMDS[(Ie)$cmd]} )); then
        fail "Command '$cmd' was wrapped but shouldn't be."
    fi
}

cleanup() {
    _island_precmd
    # Clear aliases and functions created during test
    unalias -m '*' 2>/dev/null || true
}

echo "Running Zsh hook tests..."

echo "Test 1: Simple external command (ls)"
cleanup
_island_wrap_cmd "ls"
assert_wrapped "ls"

echo "Test 2: Simple alias (ll -> ls -l)"
cleanup
alias ll="ls -l"
_island_wrap_cmd "ll"
assert_wrapped "ls"
assert_not_wrapped "ll"

echo "Test 3: Recursive alias (la -> ll -> ls)"
cleanup
alias l="ls"
alias ll="l -l"
alias la="ll -a"
_island_wrap_cmd "la"
assert_wrapped "ls"
assert_not_wrapped "l"
assert_not_wrapped "ll"
assert_not_wrapped "la"

echo "Test 4: Alias with env var (mygrep -> GREP_COLOR=... grep)"
cleanup
alias grep="grep --color=auto"
alias mygrep="GREP_COLOR='1;32' grep"
_island_wrap_cmd "mygrep"
assert_wrapped "grep"

echo "Test 5: Alias with precommand modifier (exec_ls -> exec ls)"
cleanup
alias exec_ls="exec ls"
_island_wrap_cmd "exec_ls"
assert_wrapped "ls"
assert_not_wrapped "exec"

echo "Test 6: Complex alias (complex -> VAR=1 nocorrect ls)"
cleanup
alias complex="VAR1=1 VAR2=2 nocorrect exec ls -la"
_island_wrap_cmd "complex"
assert_wrapped "ls"
assert_not_wrapped "nocorrect"
assert_not_wrapped "exec"

echo "Test 7: Existing function"
cleanup
function myfunc() { echo "func"; }
_island_wrap_cmd "myfunc"
assert_not_wrapped "myfunc"
unfunction myfunc

echo "Test 8: Builtin (cd)"
cleanup
_island_wrap_cmd "cd"
assert_not_wrapped "cd"

echo "Test 9: Non-existent command"
cleanup
_island_wrap_cmd "nonexistentcommand"
assert_not_wrapped "nonexistentcommand"

echo "Test 10: Command with path (/usr/bin/ls)"
cleanup
_island_wrap_cmd "/usr/bin/ls"
assert_not_wrapped "/usr/bin/ls"

echo "Test 11: Idempotency"
source "$HOOK_SCRIPT"
if [[ "$(type -w _island_accept_line)" != "_island_accept_line: function" ]]; then
    fail "_island_accept_line is not a function after re-sourcing"
fi

echo "Test 12: Alias collision (ls is alias, wrap ls)"
cleanup
alias ls="ls --color=auto"
_island_wrap_cmd "ls"
assert_wrapped "ls"

echo "Test 13: Alias with eval (eval_ls -> eval ls)"
cleanup
alias eval_ls="eval ls"
_island_wrap_cmd "eval_ls"
assert_wrapped "ls"

echo "Test 14: nosandbox (nosandbox ls -> ls)"
cleanup
_island_accept_line_wrapper() {
    BUFFER="nosandbox ls"
    _island_accept_line
}

# Mock zle to capture the buffer modification
zle() {
    if [[ "$1" == "_island_orig_accept_line" ]]; then
        echo "DEBUG: Final BUFFER: $BUFFER"
    fi
}

# We need to mock _island_wrap_cmd to verify it's NOT called for ls
_island_wrap_cmd() {
    echo "DEBUG: _island_wrap_cmd called for $1"
    _ISLAND_WRAPPED_CMDS+=("$1")
}

BUFFER="nosandbox ls"
_island_accept_line
if (( ${+_ISLAND_WRAPPED_CMDS} )) && (( ${_ISLAND_WRAPPED_CMDS[(Ie)ls]} )); then
    fail "nosandbox ls resulted in wrapping ls"
fi

# nosandbox is an alias to empty string, so it stays in the buffer but
# effectively disappears during execution.
if [[ "$BUFFER" != "nosandbox ls" ]]; then
    fail "Buffer was unexpectedly modified. Buffer: '$BUFFER'"
fi

echo "All tests passed!"
