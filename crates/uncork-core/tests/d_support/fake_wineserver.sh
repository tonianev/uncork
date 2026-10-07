#!/bin/sh
# A fake wineserver for Uncork's tests: logs its arguments. Like the real
# one, --kill leaves the registry (and so a killed client's pid) alone.
state='@STATE@'
printf 'wineserver %s | WINEPREFIX=%s\n' "$*" "$WINEPREFIX" >> "$state/calls.log"
if [ "$1" = --kill ]; then
    : > "$state/killed"
fi
exit 0
