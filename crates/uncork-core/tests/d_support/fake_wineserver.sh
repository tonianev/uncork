#!/bin/sh
# A fake wineserver for Uncork's tests: logs its arguments. @STATE@/server
# stands for a running wineserver (the fake wine creates it). Like the real
# one, --kill leaves the registry (and so a killed client's pid) alone and
# exits 1 when nothing was running; -k0 only asks whether one runs.
state='@STATE@'
printf 'wineserver %s | WINEPREFIX=%s\n' "$*" "$WINEPREFIX" >> "$state/calls.log"
case "$1" in
    -k0)
        [ -f "$state/server" ] && exit 0
        exit 1
        ;;
    --kill)
        [ -f "$state/server" ] || exit 1
        rm -f "$state/server"
        : > "$state/killed"
        ;;
    --wait)
        rm -f "$state/server"
        ;;
esac
exit 0
