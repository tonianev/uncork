#!/usr/bin/env bash
# verify_catalog_pins.sh: download every archive in runtime/catalog.toml and
# check its size and SHA-256 against the pin. For maintainers reviewing a
# catalog-pin pull request or auditing the catalog; CI never runs it.
#
# It downloads hundreds of megabytes from the publishers (GitHub release
# assets today) into a temporary directory and deletes each file after
# checking it. Nothing is installed or executed.
#
# Usage: scripts/verify_catalog_pins.sh [--only KIND[@VERSION]] [CATALOG]
#   --only wine            check only the wine entries
#   --only dxmt@0.80       check one entry
#   CATALOG                defaults to runtime/catalog.toml
# Exit status: 0 when every checked pin matches, 1 otherwise.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
catalog="$root/runtime/catalog.toml"
only=""
while [ $# -gt 0 ]; do
  case "$1" in
    --only)
      [ $# -ge 2 ] || {
        echo "error: --only needs KIND or KIND@VERSION" >&2
        exit 2
      }
      only="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "error: unknown option $1 (see --help)" >&2
      exit 2
      ;;
    *)
      catalog="$1"
      shift
      ;;
  esac
done

command -v curl >/dev/null 2>&1 || {
  echo "error: curl not found" >&2
  exit 1
}

sha256_of() {
  local out
  if command -v shasum >/dev/null 2>&1; then
    out="$(shasum -a 256 "$1")"
  else
    out="$(sha256sum "$1")"
  fi
  printf '%s\n' "${out%% *}"
}

# The lint runs first: a malformed catalog is not worth downloading for.
entries="$("$root/scripts/check_catalog.sh" --list "$catalog")"

selected=()
total_bytes=0
while IFS=$'\t' read -r kind version size sha256 url; do
  [ -n "$kind" ] || continue
  case "$only" in
    '' | "$kind" | "$kind@$version") ;;
    *) continue ;;
  esac
  selected+=("$kind"$'\t'"$version"$'\t'"$size"$'\t'"$sha256"$'\t'"$url")
  total_bytes=$((total_bytes + size))
done <<<"$entries"

if [ "${#selected[@]}" -eq 0 ]; then
  echo "error: no catalog entry matches --only $only" >&2
  exit 1
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/uncork-verify-pins.XXXXXX")"
trap 'rm -rf "$work"' EXIT
echo "verify_catalog_pins: ${#selected[@]} archive(s), $((total_bytes / 1048576)) MiB to download"

failures=0
for entry in "${selected[@]}"; do
  IFS=$'\t' read -r kind version size sha256 url <<<"$entry"
  file="$work/download"
  printf '%s@%s: ' "$kind" "$version"
  if ! curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --retry 3 --connect-timeout 30 --silent --show-error --output "$file" "$url"; then
    echo "FAIL (download error: $url)"
    failures=$((failures + 1))
    continue
  fi
  got_size="$(wc -c <"$file" | tr -d '[:space:]')"
  got_sha256="$(sha256_of "$file")"
  rm -f "$file"
  if [ "$got_size" != "$size" ]; then
    echo "FAIL (size $got_size, pinned $size)"
    failures=$((failures + 1))
  elif [ "$got_sha256" != "$sha256" ]; then
    echo "FAIL (sha256 $got_sha256, pinned $sha256)"
    failures=$((failures + 1))
  else
    echo "ok ($size bytes, sha256 $sha256)"
  fi
done

if [ "$failures" -gt 0 ]; then
  echo "verify_catalog_pins: $failures of ${#selected[@]} pin(s) do not match" >&2
  exit 1
fi
echo "verify_catalog_pins: all ${#selected[@]} pin(s) match"
