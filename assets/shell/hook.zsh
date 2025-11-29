#!/usr/bin/env zsh
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Island shell integration for Zsh: https://github.com/landlock-lsm/island
#
# # Usage
#
# Add this to your ~/.zshrc:
#
#   source <(island hook zsh)
#
# You can use the $_ISLAND_PROFILES array in your prompt to display the
# active Island profiles, or just to display if the current working directory is
# handled. For example:
#
#   setopt PROMPT_SUBST
#   PROMPT='%~ %B%F{yellow}${_ISLAND_PROFILES:+▣ }%f%b%# '
#
# You can also print Island context changes by adding a precmd hook like this:
#
#   typeset -g -a _ISLAND_PROFILES_BKP
#   if [[ "${_ISLAND_PROFILES[*]}" != "${_ISLAND_PROFILES_BKP[*]}" ]]; then
#     _ISLAND_PROFILES_BKP=("${_ISLAND_PROFILES[@]}")
#     echo "Updated Island environment: ${_ISLAND_PROFILES[*]}"
#   fi
#
# You can bypass the sandbox for a single command by prefixing it with `nosandbox`:
#
#   nosandbox ls /
#
# # Goal
#
# Transparently sandbox commands (e.g., `ls`, `make`) when running in an
# Island-managed directory, without changing the user experience.
#
# # How it works
#
# - Use the accept-line ZLE widget to intercept the Enter key.
# - Parse the command line to identify commands (handling pipes, aliases...).
# - For each external command, define a temporary shell function with the same
#   name.
# - This function wraps the execution in `island run` and checks funcstack to
#   ensure it only affects direct user calls (avoiding side effects in other
#   hooks).
# - A precmd hook cleans up these wrappers immediately after execution.
#
# # Features
#
# - Recursive alias resolution (e.g., `ll` -> `ls -l` -> `ls --color=auto -l`).
# - Pipeline support (sandboxes `date` and `head` in `date | head`).
# - Safety checks (ignores shell functions, builtins, and non-direct calls).
# - Idempotency: This script is safe to source multiple times.
#
# # Limitations
#
# - Existing Functions: If a command is already a shell function, do not wrap it
#   to avoid breaking it.
# - Sourced scripts: Commands inside sourced files (`source script.sh`) are not
#   intercepted.
# - Eval and subshells: Complex commands hidden inside eval or dynamic
#   subshells might be missed.
# - Paths: Commands invoked via path (e.g., ./script.sh, /bin/ls) cannot
#   be wrapped with functions (names can't contain slashes). Fall back to
#   modifying $BUFFER for these, which is visible in shell history.

# Ensure clean state if re-sourced, especially for the accept-line wrapper.
if functions _island_unhook >/dev/null; then
    _island_unhook
fi

function _island_chpwd() {
    # Use standard Zsh options.
    emulate -L zsh

    local profiles
    # Check if we are in an Island-managed directory.  Use 'command' to avoid
    # functions or aliases.
    if profiles="$(command island status 2>/dev/null)"; then
        # Use typeset -g -a to make it a global array but not exported.
        # (f) splits the output by newlines.
        typeset -g -a _ISLAND_PROFILES
        _ISLAND_PROFILES=("${(f)profiles}")
    else
        unset _ISLAND_PROFILES
    fi
}

function _island_wrap_cmd() {
    emulate -L zsh

    # (Q) removes one level of quoting (e.g. 'ls' -> ls)
    local cmd="${(Q)1}"

    if [[ -z "$cmd" || "$cmd" == "island" || "$cmd" == \#* ]]; then
        return
    fi

    # Resolve aliases recursively.  (A) declares an associative array to prevent
    # infinite loops.
    local -A seen
    while [[ -n "${aliases[$cmd]}" ]]; do
        if [[ -n "${seen["$cmd"]}" ]]; then
            break
        fi
        seen["$cmd"]=1

        # (z) splits the alias definition into shell words.
        local -a words=("${(z)aliases[$cmd]}")

        # Skip assignments (VAR=val) and precommand modifiers.
        while (( $#words > 0 )); do
            local word="${words[1]}"
            if [[ "$word" == [a-zA-Z_][a-zA-Z0-9_]*=* ]] || \
               [[ "$word" == [a-zA-Z_][a-zA-Z0-9_]*+=* ]]; then
                shift words
            elif [[ "$word" == (nocorrect|noglob|exec|eval) ]]; then
                shift words
            else
                break
            fi
        done

        if (( $#words == 0 )); then
            return
        fi

        # (Q) removes one level of quoting (e.g. 'ls' -> ls).
        cmd="${(Q)words[1]}"
    done

    if [[ "$cmd" == */* ]]; then
        return
    fi

    if [[ -n "${functions[$cmd]}" ]]; then
        return
    fi

    # 'whence -p' checks if the command exists in PATH as an external
    # executable.  Unlike 'command -v', it ignores builtins and functions.
    if ! whence -p "$cmd" >/dev/null; then
        return
    fi

    if (( ${_ISLAND_WRAPPED_CMDS[(Ie)$cmd]} )); then
        return
    fi

    # Define ephemeral wrapper function.  We use 'command' to bypass the wrapper
    # itself and avoid infinite recursion.  We use ${(qq)cmd} to quote the
    # command name for the eval string to prevent alias expansion during definition.
    eval "function ${(qq)cmd}() {
        if [[ \${#funcstack[@]} -gt 1 ]]; then
            command ${(qq)cmd} \"\$@\"
            return
        fi
        command island run -- ${(qq)cmd} \"\$@\"
    }"
    _ISLAND_WRAPPED_CMDS+=("${(q)cmd}")
}

function _island_accept_line() {
    emulate -L zsh

    # Only run if Island is active.
    if (( ${#_ISLAND_PROFILES} == 0 )); then
        zle _island_orig_accept_line
        return
    fi

    typeset -g -a _ISLAND_WRAPPED_CMDS=()
    local word expecting=1 modified=0
    local -a new_buffer_words

    # (z) splits the buffer into words respecting shell quoting rules.
    for word in "${(z)BUFFER}"; do
        if (( expecting )); then
            if [[ "$word" == *=* ]]; then
                new_buffer_words+=("$word")
                continue
            fi

            # Handle paths (containing /) by modifying the buffer.
            if [[ "$word" == */* ]]; then
                if [[ -x "${(Q)word}" ]] || whence -p "${(Q)word}" >/dev/null; then
                    new_buffer_words+=("island" "run" "--" "$word")
                    modified=1
                else
                    new_buffer_words+=("$word")
                fi
            else
                _island_wrap_cmd "$word"
                new_buffer_words+=("$word")
            fi
            expecting=0
        elif [[ "$word" == ("|"|"|&"|";"|"&"|"&&"|"||") ]]; then
            new_buffer_words+=("$word")
            expecting=1
        else
            new_buffer_words+=("$word")
        fi
    done

    if (( modified )); then
        BUFFER="${new_buffer_words}"
    fi

    zle _island_orig_accept_line
}

# Cleanup hook to ensure no wrappers are left behind.
function _island_precmd() {
    emulate -L zsh

    local cmd
    for cmd in "${_ISLAND_WRAPPED_CMDS[@]-}"; do
        unfunction "$cmd" 2>/dev/null
    done
    unset _ISLAND_WRAPPED_CMDS
}

# User helper to run commands without Island wrapping.
#
# We use an alias to ensure shell completion works transparently (treating the
# next word as the command).
unfunction nosandbox 2>/dev/null || :
alias nosandbox=''

if (( $+functions[compdef] )); then
    compdef _precommand nosandbox
fi

# Wrap the island command to update the environment immediately.
function island() {
    command island "$@"
    local ret=$?
    _island_chpwd
    return $ret
}

# User helper to unhook Island integration.
function _island_unhook() {
    emulate -L zsh

    add-zsh-hook -d chpwd _island_chpwd
    add-zsh-hook -d precmd _island_precmd

    zle -A _island_orig_accept_line accept-line
    zle -D _island_orig_accept_line

    _island_precmd

    unfunction _island_accept_line 2>/dev/null || :
    unfunction _island_chpwd 2>/dev/null || :
    unfunction _island_precmd 2>/dev/null || :
    unfunction _island_unhook 2>/dev/null || :
    unfunction _island_wrap_cmd 2>/dev/null || :
    unfunction island 2>/dev/null || :

    unalias nosandbox 2>/dev/null || :
    if (( $+functions[compdef] )); then
        compdef -d nosandbox 2>/dev/null || :
    fi

    unset _ISLAND_PROFILES
    unset _ISLAND_WRAPPED_CMDS
}

autoload -Uz add-zsh-hook
add-zsh-hook chpwd _island_chpwd
add-zsh-hook precmd _island_precmd

# Wrap the accept-line widget to intercept commands.
if ! zle -l _island_orig_accept_line; then
    zle -A accept-line _island_orig_accept_line
fi
zle -N accept-line _island_accept_line

_island_chpwd
