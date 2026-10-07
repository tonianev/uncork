#!/usr/bin/env bash
# check_catalog.sh: offline lint for runtime/catalog.toml.
#
# Every [[component]] entry must pin one archive: an https URL ending in its
# archive format, a 64-hex lowercase SHA-256, a positive size, a license and
# https links to its source code and homepage. Kinds are wine, dxmt or dxvk
# (D3DMetal is never downloadable), (kind, version) pairs are unique, at
# most one entry per kind is recommended, and unknown keys are rejected as
# Uncork's own parser rejects them. Nothing is downloaded; to check that the
# pins match what the publishers serve, see scripts/verify_catalog_pins.sh.
#
# Usage: scripts/check_catalog.sh [--list] [CATALOG]
#   CATALOG  defaults to runtime/catalog.toml
#   --list   after checking, print one tab-separated line per entry:
#            kind, version, size, sha256, url
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
list=0
catalog="$root/runtime/catalog.toml"
while [ $# -gt 0 ]; do
  case "$1" in
    --list) list=1 ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "error: unknown option $1 (see --help)" >&2
      exit 2
      ;;
    *) catalog="$1" ;;
  esac
  shift
done
[ -f "$catalog" ] || {
  echo "error: $catalog not found" >&2
  exit 1
}

# One awk pass. The catalog is flat TOML: `[[component]]` headers and
# `key = value` lines, where strings never contain an escaped quote.
awk -v list="$list" '
  function err(line, msg) {
    printf "%s:%d: %s\n", FILENAME, line, msg > "/dev/stderr"
    errors++
  }
  function is_hex64(s) { return length(s) == 64 && s ~ /^[0-9a-f]+$/ }
  function is_https(s) { return s ~ /^https:\/\/[^ \t"]+$/ }
  # value_of(raw): the value of a `key = value` right-hand side, with string
  # quotes and a trailing comment removed.
  function value_of(raw) {
    sub(/^[ \t]+/, "", raw)
    if (substr(raw, 1, 1) == "\"") {
      raw = substr(raw, 2)
      return substr(raw, 1, index(raw, "\"") - 1)
    }
    sub(/[ \t]*#.*$/, "", raw)
    sub(/[ \t]+$/, "", raw)
    return raw
  }
  function finish_entry(   key, n, required, i) {
    if (!in_entry) return
    n = split("kind version url sha256 size archive license source_code homepage", required, " ")
    for (i = 1; i <= n; i++)
      if (!(required[i] in v)) err(start, "component is missing `" required[i] "`")
    if ("kind" in v) {
      if (v["kind"] == "d3dmetal")
        err(start, "d3dmetal is never downloadable; it is imported from the user'"'"'s own GPTK")
      else if (v["kind"] !~ /^(wine|dxmt|dxvk)$/)
        err(start, "unknown kind `" v["kind"] "` (wine, dxmt or dxvk)")
    }
    if (("version" in v) && v["version"] !~ /^[A-Za-z0-9][A-Za-z0-9._+-]*$/)
      err(start, "version `" v["version"] "` must be letters, digits and . _ + - (it becomes a directory name)")
    if (("url" in v) && !is_https(v["url"]))
      err(start, "url must be https: " v["url"])
    if (("url" in v) && ("archive" in v) && substr(v["url"], length(v["url"]) - length(v["archive"])) != "." v["archive"])
      err(start, "url does not end in ." v["archive"] ": " v["url"])
    if (("sha256" in v) && !is_hex64(v["sha256"]))
      err(start, "sha256 must be 64 lowercase hex characters, got `" v["sha256"] "`")
    if (("size" in v) && (v["size"] !~ /^[0-9]+$/ || v["size"] + 0 <= 0))
      err(start, "size must be a positive number of bytes, got `" v["size"] "`")
    if (("archive" in v) && v["archive"] !~ /^tar\.(gz|xz)$/)
      err(start, "archive must be tar.gz or tar.xz, got `" v["archive"] "`")
    if (("license" in v) && v["license"] == "")
      err(start, "license must not be empty")
    if (("source_code" in v) && !is_https(v["source_code"]))
      err(start, "source_code must be an https link: " v["source_code"])
    if (("homepage" in v) && !is_https(v["homepage"]))
      err(start, "homepage must be an https link: " v["homepage"])
    if (("recommended" in v) && v["recommended"] !~ /^(true|false)$/)
      err(start, "recommended must be true or false")
    if (("kind" in v) && ("version" in v)) {
      key = v["kind"] "@" v["version"]
      if (key in entries) err(start, "duplicate entry " key " (first at line " entries[key] ")")
      entries[key] = start
    }
    if (v["recommended"] == "true") {
      if (v["kind"] in recommended)
        err(start, "a second recommended " v["kind"] " (first at line " recommended[v["kind"]] ")")
      recommended[v["kind"]] = start
    }
    count++
    rows[count] = v["kind"] "\t" v["version"] "\t" v["size"] "\t" v["sha256"] "\t" v["url"]
    in_entry = 0
  }
  BEGIN {
    n = split("kind version url sha256 size archive strip_prefix license source_code homepage features recommended notes", names, " ")
    for (i = 1; i <= n; i++) allowed[names[i]] = 1
  }
  /^[ \t]*(#|$)/ { next }
  /^\[\[component\]\][ \t]*(#.*)?$/ {
    finish_entry()
    in_entry = 1
    start = FNR
    split("", v)
    next
  }
  /^\[/ { finish_entry(); err(FNR, "unexpected table " $0); next }
  /^[A-Za-z_][A-Za-z0-9_]*[ \t]*=/ {
    key = $0
    sub(/[ \t]*=.*/, "", key)
    raw = $0
    sub(/^[^=]*=/, "", raw)
    if (!in_entry) {
      if (key == "schema") schema = value_of(raw)
      else err(FNR, "unknown top-level key `" key "`")
      next
    }
    if (!(key in allowed)) { err(FNR, "unknown key `" key "`"); next }
    if (key in v) { err(FNR, "duplicate key `" key "`"); next }
    v[key] = value_of(raw)
    next
  }
  # Continuation lines of a multi-line array (features = [ ... ]).
  /^[ \t]*("[^"]*"[ \t]*,?[ \t]*)+(#.*)?$/ || /^[ \t]*\][ \t]*$/ { next }
  { err(FNR, "cannot parse: " $0) }
  END {
    finish_entry()
    if (schema != "1") err(1, "schema must be 1, got `" schema "`")
    if (count == 0) err(1, "no [[component]] entries")
    if (errors) {
      printf "check_catalog: %d problem(s) in %s\n", errors, FILENAME > "/dev/stderr"
      exit 1
    }
    if (list) for (i = 1; i <= count; i++) print rows[i]
    else printf "check_catalog: %d components ok\n", count
  }
' "$catalog"
