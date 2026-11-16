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

# Runs a command line at the top level of a new shell (wrappers do not sandbox
# when they are called from a function, such as a test), with a fake island
# command, and checks the number of commands that went through `island run`.
#
# $1: expected number of sandboxed commands
# $2: command line
# $3: optional setup code, run before the command line is handled
# $4: optional directory where the fake island reports no profile
assert_sandboxed() {
    local expected="$1" line="$2" setup="${3:-}" inactive="${4:-/nonexistent}"
    local dir actual
    dir="$(mktemp -d)"

    cat > "$dir/island" <<'EOF'
#!/bin/sh
case "$1" in
    status)
        case "$PWD" in
            "$ISLAND_TEST_INACTIVE"*) exit 1;;
        esac
        echo default;;
    run)
        # Skip "run --".
        shift 2
        echo "$1" >> "$ISLAND_TEST_LOG"
        exec "$@";;
esac
EOF
    chmod +x "$dir/island"
    : > "$dir/log"

    local script="source ${(qq)HOOK_SCRIPT:A}
$setup
_island_preexec ${(qq)line} ${(qq)line} ${(qq)line}
$line"
    ISLAND_TEST_LOG="$dir/log" ISLAND_TEST_INACTIVE="$inactive" \
        PATH="$dir:$PATH" zsh -f -c "$script" >/dev/null 2>&1 || true

    actual="$(wc -l < "$dir/log")"
    if [[ "$actual" -ne "$expected" ]]; then
        tap_debug "Sandboxed: $(tr '\n' ' ' < "$dir/log")"
        rm -rf "$dir"
        tap_fail "'$line' sandboxed $actual command(s) instead of $expected"
    fi
    rm -rf "$dir"
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
    assert_wrapped "/usr/bin/ls"
    tap_pass
}

test_idempotency() {
    tap_start "Idempotency"
    source "$HOOK_SCRIPT"
    if [[ "$(type -w _island_preexec)" != "_island_preexec: function" ]]; then
        tap_fail "_island_preexec is not a function after re-sourcing"
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
    _ISLAND_PROFILES=("default")
    _island_preexec "nosandbox ls" "nosandbox ls" "nosandbox ls"

    # ls is wrapped, but the wrapper does not sandbox when it is called from
    # the nosandbox function.
    assert_wrapped "ls"
    assert_not_wrapped "nosandbox"
    if ! functions nosandbox >/dev/null; then
        tap_fail "nosandbox function does not exist"
    fi
    if [[ "$(nosandbox echo ok)" != "ok" ]]; then
        tap_fail "nosandbox did not run the command"
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

    # Execute a new command to trigger the preexec logic.
    _island_preexec "ls" "ls" "ls"

    if functions old_cmd >/dev/null; then
        tap_fail "old_cmd wrapper was not cleaned up"
    fi

    assert_wrapped "ls"

    tap_pass
}

test_option_words() {
    tap_start "Words starting with a dash"
    cleanup
    _ISLAND_PROFILES=("default")
    local out
    out="$(_island_preexec "ls -h -o" "ls -h -o" "ls -h -o" 2>&1)"
    if [[ -n "$out" ]]; then
        tap_fail "Unexpected output: $out"
    fi
    tap_pass
}

test_run_simple() {
    tap_start "Run: simple commands, pipelines and lists"
    assert_sandboxed 1 "ls"
    assert_sandboxed 2 "ls | cat"
    assert_sandboxed 2 "ls && ls"
    assert_sandboxed 1 "FOO=1 ls"
    assert_sandboxed 1 "2>/dev/null ls"
    tap_pass
}

test_run_precommand() {
    tap_start "Run: precommand modifiers"
    assert_sandboxed 1 "noglob ls"
    assert_sandboxed 1 "nocorrect ls"
    assert_sandboxed 1 "command ls"
    assert_sandboxed 1 "! ls"
    assert_sandboxed 0 "command -v ls"
    tap_pass
}

test_run_compound() {
    tap_start "Run: control structures and groups"
    assert_sandboxed 1 "for i in 1; do ls; done"
    assert_sandboxed 1 "if ls; then :; fi"
    assert_sandboxed 1 "{ ls }"
    assert_sandboxed 1 "( ls )"
    assert_sandboxed 1 "(ls)"
    tap_pass
}

test_run_substitution() {
    tap_start "Run: substitutions"
    assert_sandboxed 1 'echo $(ls)'
    assert_sandboxed 1 'echo "$(ls)"'
    assert_sandboxed 1 'echo `ls`'
    assert_sandboxed 2 'cat <(ls)'
    tap_pass
}

test_run_eval() {
    tap_start "Run: eval"
    assert_sandboxed 1 "eval ls"
    assert_sandboxed 1 "eval 'ls -a'"
    tap_pass
}

test_run_path() {
    tap_start "Run: commands invoked via path"
    local ls_path="$(whence -p ls)"
    local ls_dir="${ls_path:h}"
    assert_sandboxed 1 "$ls_path"
    assert_sandboxed 1 "=ls"
    assert_sandboxed 1 "~lsdir/ls" "hash -d lsdir=${(qq)ls_dir}"
    assert_sandboxed 1 "cd ${(qq)ls_dir} && ./ls"
    tap_pass
}

test_run_parameter() {
    tap_start "Run: parameters used as commands"
    local ls_path="$(whence -p ls)"
    assert_sandboxed 1 '$LS' "LS=ls"
    assert_sandboxed 1 '${LS}' "LS=ls"
    assert_sandboxed 1 '$LSDIR/ls' "LSDIR=${(qq)ls_path:h}"
    tap_pass
}

test_run_chdir() {
    tap_start "Run: directory change in the command line"
    local inactive
    inactive="$(mktemp -d)"
    # Enters an Island-managed directory.
    assert_sandboxed 1 "cd / && ls" "cd ${(qq)inactive}" "$inactive"
    # Leaves an Island-managed directory.
    assert_sandboxed 0 "cd ${(qq)inactive} && ls" "cd /" "$inactive"
    # Stays out of any Island-managed directory.
    assert_sandboxed 0 "ls" "cd ${(qq)inactive}" "$inactive"
    rmdir "$inactive"
    tap_pass
}

test_run_not_sandboxed() {
    tap_start "Run: commands that must not be sandboxed"
    assert_sandboxed 0 "nosandbox ls"
    assert_sandboxed 0 "nosandbox $(whence -p ls)"
    assert_sandboxed 0 "echo ls"
    assert_sandboxed 0 "myfunc" "function myfunc() { ls }"
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
    test_option_words
    test_run_simple
    test_run_precommand
    test_run_compound
    test_run_substitution
    test_run_eval
    test_run_path
    test_run_parameter
    test_run_chdir
    test_run_not_sandboxed
)

tap_run "$@"
