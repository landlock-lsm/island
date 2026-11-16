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
# - Use a preexec hook to get the command line that is about to be executed,
#   with aliases and history references already expanded.
# - Do not try to identify which words are commands: Zsh does that when it
#   executes the line. For each word that could be an external command
#   (including words nested in quotes or substitutions), define a temporary
#   shell function with the same name. A function for a word that is not
#   executed is harmless.
# - This function wraps the execution in `island run` and checks
#   zsh_eval_context to ensure it only affects direct user calls, i.e. it is
#   not called by another function (avoiding side effects in other hooks).
#   Unlike funcstack, this does not count eval. It also checks that Island
#   is active when it is called, which handles directory changes in the
#   middle of a command line.
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
# - Dynamic commands: Command names computed at run time (e.g., `$(echo ls)`
#   as a command, or `eval "$var"`) are missed.
# - Exec and history: `exec cmd`, `r` and `fc -e -` are not intercepted.

# Ensure clean state if re-sourced.
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

    # The command line being executed just entered an Island-managed directory
    # (e.g., `cd project && make`): wrap the rest of it.
    if (( ${#_ISLAND_PROFILES} )) && [[ -n "${_ISLAND_LINE-}" ]] && \
       (( ! ${#_ISLAND_WRAPPED_CMDS} )); then
        _island_wrap_words "$_ISLAND_LINE" 0
    fi
}

# Wrap the command builtin, which would otherwise bypass the wrapper functions
# (e.g., `command ls`).
function _island_wrap_command() {
    if [[ -n "${functions[command]}" ]]; then
        return
    fi

    function command() {
        if (( ${zsh_eval_context[(i)shfunc]} != ${zsh_eval_context[(I)shfunc]} )) ||
           [[ -z "${_ISLAND_PROFILES-}" || $# -eq 0 || "$1" == -* ]]; then
            builtin command "$@"
            return
        fi
        builtin command island run -- "$@"
    }
    _ISLAND_WRAPPED_CMDS+=("command")
}

function _island_wrap_cmd() {
    emulate -L zsh
    setopt extended_glob

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

    if [[ "$cmd" == "command" ]]; then
        _island_wrap_command
        return
    fi

    # Expand the command the same way Zsh will, because the wrapper function
    # must have the name of the expanded command.
    if [[ "$cmd" == '='[^=/]* ]]; then
        # =ls -> /usr/bin/ls
        cmd="${commands[${cmd#=}]-}"
    elif [[ "$cmd" == '~'*/* ]]; then
        # ~/bin/tool, ~user/bin/tool, ~named-dir/tool
        local dir="${${cmd%%/*}#\~}"
        if [[ -n "$dir" ]]; then
            dir="${nameddirs[$dir]:-${userdirs[$dir]-}}"
        else
            dir="$HOME"
        fi
        cmd="${dir:+$dir/${cmd#*/}}"
    elif [[ "$cmd" == *\$* ]]; then
        # $EDITOR, ${HOME}/bin/tool.  Only expand plain parameter references,
        # to make sure that (e) cannot run any code.
        local rest="${cmd//\$\{[[:IDENT:]]##\}/}"
        rest="${rest//\$[[:IDENT:]]##/}"
        if [[ "$rest" == *[\$\`]* ]]; then
            return
        fi
        cmd="${(e)cmd}"
    fi

    if [[ -z "$cmd" ]]; then
        return
    fi

    if [[ "$cmd" == */* ]]; then
        # Function names can contain slashes.  Do not wrap directories (to
        # keep AUTO_CD working) nor existing files that are not executable.  A
        # missing file may be created by a previous command, or be relative to
        # a directory entered by a previous command.
        if [[ -d "$cmd" ]] || [[ -e "$cmd" && ! -x "$cmd" ]]; then
            return
        fi
    fi

    if [[ -n "${functions[$cmd]}" ]]; then
        return
    fi

    # A builtin takes precedence over an external command with the same name
    # (e.g., echo, kill).
    if [[ -n "${builtins[$cmd]}" ]]; then
        return
    fi

    # 'whence -p' checks if the command exists in PATH as an external
    # executable.  Unlike 'command -v', it ignores builtins and functions.
    if [[ "$cmd" != */* ]] && ! whence -p -- "$cmd" >/dev/null; then
        return
    fi

    # Check if the command is already wrapped.  (I) returns the index of the
    # match, or 0 if not found.  (e) performs an exact string match (no
    # globbing).
    if (( ${_ISLAND_WRAPPED_CMDS[(Ie)${(q)cmd}]} )); then
        return
    fi

    # Define ephemeral wrapper function.  We use 'command' to bypass the wrapper
    # itself and avoid infinite recursion.  We use ${(qq)cmd} to quote the
    # command name for the eval string to prevent alias expansion during definition.
    eval "function ${(qq)cmd}() {
        if (( \${zsh_eval_context[(i)shfunc]} != \${zsh_eval_context[(I)shfunc]} )) ||
           [[ -z \"\${_ISLAND_PROFILES-}\" ]]; then
            command ${(qq)cmd} \"\$@\"
            return
        fi
        command island run -- ${(qq)cmd} \"\$@\"
    }"
    _ISLAND_WRAPPED_CMDS+=("${(q)cmd}")
}

# Wrap all the words of a piece of shell code that could be commands.
function _island_wrap_words() {
    emulate -L zsh

    local word inner

    # Guard against unexpected recursion.
    if (( $2 > 8 )); then
        return
    fi

    # (z) splits the text into words respecting shell quoting rules.
    for word in "${(z)1}"; do
        _island_wrap_cmd "$word"

        # Look for commands nested in the word: quoted strings (e.g., eval
        # "ls -l"), substitutions (e.g., $(ls), <(ls)), or assignments.
        inner="${${(Q)word}//[\$\`\(\)\<\>\{\}\"\'=]/ }"
        if [[ "$inner" != "$word" && "$inner" != "$1" ]]; then
            _island_wrap_words "$inner" $(( $2 + 1 ))
        fi
    done
}

# $1 is the history line and $3 is the full text that is about to be executed,
# with aliases expanded.
function _island_preexec() {
    emulate -L zsh

    # Safety cleanup in case precmd was skipped (e.g., because a wrapped command
    # was called, or the hook has been removed) or interrupted, before the
    # following reset.
    if (( ${#_ISLAND_WRAPPED_CMDS} )); then
        _island_precmd
    fi

    typeset -g -a _ISLAND_WRAPPED_CMDS=()
    typeset -g _ISLAND_LINE="${3:-${1-}}"

    # Only run if Island is active.  Otherwise, _island_chpwd will take care of
    # a directory change.
    if (( ${#_ISLAND_PROFILES} )); then
        _island_wrap_words "$_ISLAND_LINE" 0
    fi
}

# Cleanup hook to ensure no wrappers are left behind.
function _island_precmd() {
    emulate -L zsh

    local cmd
    for cmd in "${_ISLAND_WRAPPED_CMDS[@]}"; do
        [[ -n "$cmd" ]] || continue
        unfunction "${(Q)cmd}"
    done
    unset _ISLAND_WRAPPED_CMDS
    unset _ISLAND_LINE
}

# User helper to run commands without Island wrapping.
#
# We use a function because wrappers only sandbox direct user calls.  The alias
# (ending with a space) keeps alias expansion for the next word.
function nosandbox() {
    "$@"
}
alias nosandbox='nosandbox '

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
    add-zsh-hook -d preexec _island_preexec

    _island_precmd

    unfunction _island_chpwd 2>/dev/null || :
    unfunction _island_precmd 2>/dev/null || :
    unfunction _island_preexec 2>/dev/null || :
    unfunction _island_unhook 2>/dev/null || :
    unfunction _island_wrap_cmd 2>/dev/null || :
    unfunction _island_wrap_command 2>/dev/null || :
    unfunction _island_wrap_words 2>/dev/null || :
    unfunction island 2>/dev/null || :
    unfunction nosandbox 2>/dev/null || :

    unalias nosandbox 2>/dev/null || :
    if (( $+functions[compdef] )); then
        compdef -d nosandbox 2>/dev/null || :
    fi

    unset _ISLAND_PROFILES
    unset _ISLAND_WRAPPED_CMDS
    unset _ISLAND_LINE
}

autoload -Uz add-zsh-hook
add-zsh-hook chpwd _island_chpwd
add-zsh-hook precmd _island_precmd
add-zsh-hook preexec _island_preexec

_island_chpwd
