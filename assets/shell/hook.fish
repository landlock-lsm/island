#!/usr/bin/env fish
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Island shell integration for Fish: https://github.com/landlock-lsm/island
#
# # Usage
#
# Add this to your ~/.config/fish/config.fish:
#
#   source (island hook fish | psub)
#
# You can use the $_ISLAND_PROFILES array in your prompt to display the
# active Island profiles, or just to display if the current working directory is
# handled. For example:
#
# functions --copy fish_prompt _orig_fish_prompt
# function fish_prompt
#     if test -n "$_ISLAND_PROFILES"
#         echo -sn (set_color blue) "island: " "$_ISLAND_PROFILES" " "
#     end
#     _orig_fish_prompt
# end
#
# # Goal
#
# Transparently sandbox commands (e.g., `ls`, `make`) when running in an
# Island-managed directory, without changing the user experience.
#
# # Features
# - Pipeline support (sandboxes `date` and `head` in `date | head`).
# - Command chaining support (sandboxes `ls` and `echo` in `ls && echo
#   done` and its variants).
# - Idempotency: This script is safe to source multiple times.
#
# # Limitations
#
# - No handling of command substitutions.
#
# - Since aliases are defined as functions in fish, they are not easy to
#   resolve automatically.  For alias-like behavior, use `abbr` instead,
#   which will automatically expand before execution.
#
# - No support for functions.  This includes things like the built-in `ls`
#   and `ll`, `man`, etc.  (They will still work, but won't be sandboxed.)
#
# - fish functions does not support streaming output when inside a
#   pipeline (all outputs are buffered and printed at once at exit) [1][2].
#   This means that with this hook, a command like
#
#     base64 /dev/urandom | head
#
#   will show nothing and eventually run out of memory, since `base64`
#   (and `head`) is redefined to be a function.  `nosandbox` does not help
#   here, to work around this, you need to use `command` on either side:
#
#     command base64 /dev/urandom | head
#     base64 /dev/urandom | command head
#
#   alternatively, to still sandbox both end of the pipe, you can manually
#   start a one-off shell:
#
#     fish -c 'base64 /dev/urandom | head'
#
#  [1]: https://github.com/fish-shell/fish-shell/issues/1396
#  [2]: https://github.com/fish-shell/fish-shell/issues/5635
#
# # Note
# - Fish has no ${(z)BUFFER} equivalent (`commandline --tokens-expanded` /
#   `commandline -o` drops operators), and so this file implements a
#   best-effort parser to handle quoted strings, operators, etc.

# Ensure clean state if re-sourced.
if functions -q _island_unhook
    _island_unhook
end

function _island_chpwd --on-variable PWD
    set -l profiles (command island status 2>/dev/null)
    if test $status -eq 0
        set -g _ISLAND_PROFILES $profiles
    else
        set -e _ISLAND_PROFILES
    end
end

function nosandbox
    if test (count $argv) -eq 0
        return 0
    end
    command $argv
end
complete -c nosandbox -w command

function _island_wrap_cmd --argument-names cmd
    if test -z "$cmd"
        return
    end
    if test "$cmd" = "island"
        return
    end

    set -l cmd_type (type -t -- $cmd 2>/dev/null)
    if test "$cmd_type" = "function" -o "$cmd_type" = "builtin"
        return
    end
    if test "$cmd_type" = ""
        return
    end

    if set -q _ISLAND_WRAPPED_CMDS[1]
        if contains -- $cmd $_ISLAND_WRAPPED_CMDS
            return
        end
    end

    set -l escaped (string escape -- $cmd)

    eval "
    function $escaped --wraps $escaped
        command island run -- $escaped \$argv
    end
    "
    if test $status -ne 0
        return 1
    end

    set -g _ISLAND_WRAPPED_CMDS $cmd $_ISLAND_WRAPPED_CMDS
end

function _island_accept_line
    commandline --is-valid
    set -l cl_status $status
    if test $cl_status -ne 0
        return
    end

    if not set -q _ISLAND_PROFILES[1]
        return
    end

    if set -q _ISLAND_WRAPPED_CMDS[1]
        for cmd in $_ISLAND_WRAPPED_CMDS
            functions -e -- $cmd
        end
    end
    set -g _ISLAND_WRAPPED_CMDS

    set -l input_lines (commandline --current-buffer)

    # If the user has typed an abbr, it might not have expanded yet.  This
    # will result in us not "catching" the expanded command, which is
    # unsafe.  Therefore we bail out and force the abbr expansion to
    # happen first.  Due to the async nature of `commandline -f`, we will
    # have to force the user to press enter again.  (We could technically
    # parse the abbr --show output and do the expansion ourselves, but for
    # now this is fine)

    for line in $input_lines
        if abbr --query "$line" >/dev/null 2>&1
            commandline --function expand-abbr repaint
            return 1
        end
    end

    # If the completion pager is active, let the default behavior handle
    # it (since if we modify the commandline, the pager will disappear,
    # causing the command to execute when the user only intends to select
    # a completion).
    if commandline --paging-mode
        return
    end

    set -l output_lines
    set -l curr_line_out ""
    set -l curr_token ""
    set -l expecting_cmd 1
    set -l in_squote 0
    set -l in_dquote 0
    set -l escaped 0
    set -l modified 0
    set -l curr_cmd_nosandbox 0

    function _island_append_token_to_out --no-scope-shadowing
        set curr_line_out "$curr_line_out$curr_token"
        set curr_token ""
    end

    function _island_process_curr_token --no-scope-shadowing
        if test -z "$curr_token"
            return
        end

        set -l unescaped (string unescape -- "$curr_token")

        if test $expecting_cmd -eq 1
            # This is the first token of a command.

            # First looks for "special" cases - in these cases, keep
            # expecting_cmd as 1 and return as the next token is still
            # going to be the command.

            if test "$unescaped" = "nosandbox"
                set curr_cmd_nosandbox 1
                # Preserve "nosandbox" in history.
                _island_append_token_to_out
                return
            end

            # env assignment cannot be in a quoted string, so test $curr_token
            if string match -qr '^[A-Za-z_][A-Za-z0-9_]*[+]?=.*' -- $curr_token
                _island_append_token_to_out
                return
            end

            if test "$unescaped" = "and" -o "$unescaped" = "or"
                _island_append_token_to_out
                return
            end

            # We have a normal command name now.
            set expecting_cmd 0

            if test $curr_cmd_nosandbox -eq 1
                _island_append_token_to_out
                set curr_cmd_nosandbox 0
                return
            end

            if string match -qr '/' -- "$unescaped"
                set curr_line_out "$curr_line_out""island run -- $curr_token"
                set curr_token ""
                set modified 1
                return
            end

            _island_wrap_cmd "$unescaped"
            if test $status -ne 0
                # failed to define wrapper, use `island run --` insertion instead.
                set curr_line_out "$curr_line_out""island run -- $curr_token"
                set curr_token ""
                set modified 1
                return
            end

            _island_append_token_to_out
        else
            _island_append_token_to_out
        end
    end

    for line in $input_lines
        # string sub starts at 1
        set -l i 1
        set -l len (string length -- $line)
        while test $i -le $len
            set -l ch (string sub -s $i -l 1 -- $line)

            if test $escaped -eq 1
                # we already added `\` to curr_token
                set curr_token "$curr_token$ch"
                set escaped 0
                set i (math $i + 1)
                continue
            end

            # In fish, escaping works in single and double quoted strings
            if test "$ch" = "\\"
                set escaped 1
                set curr_token "$curr_token$ch"
                set i (math $i + 1)
                continue
            end

            if test $in_squote -eq 1
                if test "$ch" = "'"
                    set in_squote 0
                end
                set curr_token "$curr_token$ch"
                set i (math $i + 1)
                continue
            end

            if test $in_dquote -eq 1
                if test "$ch" = '"'
                    set in_dquote 0
                end
                set curr_token "$curr_token$ch"
                set i (math $i + 1)
                continue
            end

            if test "$ch" = "'"
                set in_squote 1
                set i (math $i + 1)
                set curr_token "$curr_token$ch"
                continue
            end

            if test "$ch" = '"'
                set in_dquote 1
                set i (math $i + 1)
                set curr_token "$curr_token$ch"
                continue
            end

            if test "$ch" = "#"
                _island_process_curr_token
                set remaining (string sub -s $i -- $line)
                set curr_line_out "$curr_line_out$remaining"
                # skip rest of the line
                set i (math $len + 1)
                break
            end

            set -l remaining (string sub -s $i -- $line)
            set -l sep_len 0
            set -l sep_value ""

            # Order matters here - when two operators share a prefix,
            # match the longer one first.
            set -l separator_specs \
                "^;" \
                "^&&" \
                "^&\\|" \
                "^&" \
                "^\\|\\|" \
                "^\\|" \
                "^\\d+>\\|"

            for spec in $separator_specs
                set -l m (string match -r -- $spec $remaining)
                if test (count $m) -gt 0
                    set sep_value $m[1]
                    set sep_len (string length -- $sep_value)
                    break
                end
            end

            if test $sep_len -gt 0
                _island_process_curr_token
                set expecting_cmd 1
                set curr_cmd_nosandbox 0
                set curr_line_out "$curr_line_out$sep_value"
                set i (math $i + $sep_len)
                continue
            end

            # We don't need to handle things like 2>/dev/null since these
            # necessarily has to follow a space, and thus we would already
            # have separated the previous token.
            set -l redir (string match -r -- "^(>>|<<|>|<)" $remaining)
            if test (count $redir) -gt 0
                _island_process_curr_token
                set curr_line_out "$curr_line_out$redir[1]"
                set i (math $i + (string length -- $redir[1]))
                continue
            end

            if string match -qr '^[ \t]$' -- $ch
                _island_process_curr_token
                set curr_line_out "$curr_line_out$ch"
                set i (math $i + 1)
                continue
            end

            set i (math $i + 1)
            set curr_token "$curr_token$ch"
        end

        _island_process_curr_token
        set output_lines $output_lines $curr_line_out
        set curr_line_out ""
        set expecting_cmd 1
        set curr_cmd_nosandbox 0
    end

    if test $modified -eq 1
        commandline --replace -- $output_lines
    end

    return 0
end

function _island_accept_line_normal
    _island_accept_line
    if test $status -ne 0
        return
    end

    if set -q _island_orig_accept_line_normal
        eval "$_island_orig_accept_line_normal"
    else
        commandline --function execute
    end
end

function _island_accept_line_vi
    _island_accept_line
    if test $status -ne 0
        return
    end

    if set -q _island_orig_accept_line_vi
        eval "$_island_orig_accept_line_vi"
    else
        commandline --function execute
    end
end

function _island_precmd --on-event fish_prompt
    if not set -q _ISLAND_WRAPPED_CMDS[1]
        return 0
    end
    for cmd in $_ISLAND_WRAPPED_CMDS
        functions -e -- $cmd
    end
    set -e _ISLAND_WRAPPED_CMDS
end

function island
    command island $argv
    set -l ret $status
    _island_chpwd
    return $ret
end

function _island_unhook
    bind --erase \r
    bind -M insert --erase \r
    if set -q _island_orig_accept_line_normal
        bind \r $_island_orig_accept_line_normal
        set -e _island_orig_accept_line_normal
    end
    if set -q _island_orig_accept_line_vi
        bind -M insert \r $_island_orig_accept_line_vi
        set -e _island_orig_accept_line_vi
    end

    if functions -q _island_precmd
        _island_precmd
    end

    functions -e _island_accept_line
    functions -e _island_accept_line_normal
    functions -e _island_accept_line_vi
    functions -e _island_chpwd
    functions -e _island_precmd
    functions -e _island_unhook
    functions -e _island_wrap_cmd
    functions -e nosandbox
    functions -e island

    set -e _ISLAND_PROFILES
    set -e _ISLAND_WRAPPED_CMDS
    complete -c nosandbox --erase
end

if status is-interactive
    set -l old_bind (bind --user \r 2>/dev/null)
    set -l match (string match -r '^bind enter ([a-zA-Z0-9_-]+)$' -- $old_bind)

    if test (count $match) -eq 2
        set -g _island_orig_accept_line_normal $match[2]
    else
        set -e _island_orig_accept_line_normal
    end

    set -l old_bind (bind --user -M insert \r 2>/dev/null)
    set -l match (string match -r '^bind -M insert enter ([a-zA-Z0-9_-]+)$' -- $old_bind)
    if test (count $match) -eq 2
        set -g _island_orig_accept_line_vi $match[2]
    else
        set -e _island_orig_accept_line_vi
    end

    bind \r _island_accept_line_normal
    # Make it work under fish_vi_key_bindings
    bind -M insert \r _island_accept_line_vi
end

_island_chpwd
