#!/bin/sh
# A fake Wine loader for Uncork's tests: it logs every call to
# @STATE@/calls.log (arguments, working directory and the variables the
# tests check) and imitates the few Windows programs the tests run.
# Files in @STATE@ steer it:
#   server          a wineserver runs for the prefix (every call creates it,
#                   as a real Wine program starts one; see fake_wineserver.sh)
#   pid             the ActiveProcess pid `reg query` prints (absent: no key)
#   user            the ActiveProcess ActiveUser it prints (absent: no value)
#   no-signin       a started `Steam.exe` never signs in (ActiveUser stays 0)
#   slow-signin     a started `Steam.exe` leaves ActiveUser alone and signs
#                   in on the second ActiveUser query after it
#   reg-exit        exit with this status from `reg query` instead
#   steam-hangs     `Steam.exe` starts but never registers a pid
#   shutdown-works  `steam.exe -shutdown` clears the pid
#   no-steam-exe    the installer installs nothing
state='@STATE@'
# One write per call, so lines of concurrent calls never interleave.
line='wine'
for arg in "$@"; do line="$line $arg"; done
line="$line | cwd=$PWD WINEPREFIX=$WINEPREFIX WINEDEBUG=$WINEDEBUG WINEMSYNC=$WINEMSYNC WINEDLLOVERRIDES=$WINEDLLOVERRIDES DXVK_LOG_LEVEL=$DXVK_LOG_LEVEL DXMT_LOG_LEVEL=$DXMT_LOG_LEVEL USER=$USER PATH=$PATH"
printf '%s\n' "$line" >> "$state/calls.log"
: > "$state/server"
case "$1" in
    reg)
        if [ "$2" = add ]; then
            # reg add KEY /v <pid|ActiveUser> /t REG_DWORD /d <value> /f
            if [ "$5" = ActiveUser ]; then
                printf '0x%x' "$9" > "$state/user"
            else
                printf '0x%x' "$9" > "$state/pid"
            fi
            exit 0
        fi
        if [ -f "$state/reg-exit" ]; then
            exit "$(cat "$state/reg-exit")"
        fi
        # reg query KEY /v <name>
        if [ "$5" = ActiveUser ]; then
            if [ -f "$state/signin-countdown" ]; then
                left=$(( $(cat "$state/signin-countdown") - 1 ))
                if [ "$left" -le 0 ]; then
                    rm "$state/signin-countdown"
                    printf '0x1a9498de' > "$state/user"
                else
                    printf '%s' "$left" > "$state/signin-countdown"
                fi
            fi
            if [ -f "$state/pid" ] && [ -f "$state/user" ]; then
                printf '\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\\ActiveProcess\r\n    ActiveUser    REG_DWORD    %s\r\n\r\n' "$(cat "$state/user")"
                exit 0
            fi
        elif [ -f "$state/pid" ]; then
            printf '\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\\ActiveProcess\r\n    pid    REG_DWORD    %s\r\n\r\n' "$(cat "$state/pid")"
            exit 0
        fi
        echo 'reg: Unable to find the specified registry key' >&2
        exit 1
        ;;
    *SteamSetup.exe)
        if [ ! -f "$state/no-steam-exe" ]; then
            mkdir -p "$WINEPREFIX/drive_c/Program Files (x86)/Steam"
            printf 'steam' > "$WINEPREFIX/drive_c/Program Files (x86)/Steam/Steam.exe"
        fi
        ;;
    *[Ss]team.exe)
        case " $* " in
            *" -shutdown "*)
                if [ -f "$state/shutdown-works" ]; then printf '0x0' > "$state/pid"; fi
                ;;
            *)
                if [ ! -f "$state/steam-hangs" ]; then
                    printf '0x274' > "$state/pid"
                    if [ -f "$state/slow-signin" ]; then
                        printf '2' > "$state/signin-countdown"
                    elif [ -f "$state/no-signin" ]; then
                        printf '0x0' > "$state/user"
                    else
                        printf '0x1a9498de' > "$state/user"
                    fi
                fi
                ;;
        esac
        ;;
esac
exit 0
