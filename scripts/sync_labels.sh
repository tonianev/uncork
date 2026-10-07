#!/usr/bin/env bash
# sync_labels.sh: create or update the repository labels with the gh CLI.
#
# The list is hardcoded here (no yq dependency) and must match
# .github/labels.yml entry for entry; the script refuses to run when they
# differ. `gh label create --force` updates an existing label's color and
# description, so the script is idempotent. Labels that exist on GitHub but
# not here are left alone.
#
# Usage: scripts/sync_labels.sh [owner/repo]   (default: tonianev/uncork)
#        scripts/sync_labels.sh --check        only compare with labels.yml
# Requires: gh, authenticated with repo scope (not for --check).
set -euo pipefail

repo="tonianev/uncork"
check_only=0
case "${1:-}" in
  --check) check_only=1 ;;
  -h | --help)
    sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'
    exit 0
    ;;
  '') ;;
  *) repo="$1" ;;
esac
root="$(cd "$(dirname "$0")/.." && pwd)"

# name|color|description
labels=(
  "good first issue|7057ff|Small, self-contained, verifiable with \`cargo test\` on any Mac"
  "help wanted|008672|Open for anyone; often needs an Apple Silicon Mac and a copy of the game"
  "needs-design|d4c5f9|Needs a docs/ or ADR decision before code"
  "status:claimed|fbca04|Someone has commented to claim this; unclaimed again after 14 days of silence"
  "status:blocked|b60205|Waiting on another issue, PR, an upstream fix or an owner decision"
  "platform:macos|0e8a16|Needs an Apple Silicon Mac to work on or verify"
  "area:core|1d76db|crates/uncork-core, components, bottles, launch planning"
  "area:cli|1d76db|crates/uncork, the command-line interface"
  "area:steam|1d76db|crates/uncork-steam and the Windows Steam client flow"
  "area:pe|1d76db|crates/uncork-pe, Windows executable inspection"
  "area:runtime|1d76db|Wine and graphics components, runtime/catalog.toml, runtime/build-wine.sh"
  "area:profiles|1d76db|Game profiles under profiles/"
  "area:docs|1d76db|README, CONTRIBUTING and docs/"
  "area:infra|1d76db|CI, scripts, justfile, releases, dependencies"
  "kind:bug|d73a4a|Something does not work as documented"
  "kind:feature|a2eeef|New capability or change in scope"
  "kind:game-report|c5def5|How a game runs on a given Mac, runtime and backend"
  "kind:docs|0075ca|Documentation only"
  "kind:question|cfd3d7|Converted from a question; usually moves to Discussions"
  "game:rise-of-nations-extended-edition|f9d0c4|Rise of Nations: Extended Edition (Steam 287450)"
  "game:age-of-empires-2-definitive-edition|f9d0c4|Age of Empires II: Definitive Edition (Steam 813780)"
)

# labels.yml as name|color|description lines. It is written in one fixed
# shape: `- name:`, then `  color:` and `  description:` (optionally quoted).
yml_labels() {
  awk '
    function flush() { if (name != "") print name "|" color "|" desc }
    /^- name: / { flush(); name = substr($0, 9); color = ""; desc = ""; next }
    /^  color: / { color = substr($0, 10); next }
    /^  description: / { desc = substr($0, 16); gsub(/^"|"$/, "", desc); next }
    END { flush() }
  ' "$root/.github/labels.yml"
}

expected="$(printf '%s\n' "${labels[@]}")"
actual="$(yml_labels)"
if [ "$expected" != "$actual" ]; then
  echo "error: .github/labels.yml and scripts/sync_labels.sh disagree (< script, > labels.yml):" >&2
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") >&2 || true
  exit 1
fi

for entry in "${labels[@]}"; do
  IFS='|' read -r name color description <<<"$entry"
  if [ "${#name}" -gt 50 ] || [ "${#description}" -gt 100 ]; then
    echo "error: label '$name' is too long (GitHub allows 50 characters for names, 100 for descriptions)" >&2
    exit 1
  fi
  case "$color" in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
    *)
      echo "error: label '$name' has color '$color'; use 6 lowercase hex digits" >&2
      exit 1
      ;;
  esac
done

if [ "$check_only" -eq 1 ]; then
  echo "sync_labels: ${#labels[@]} labels, labels.yml in sync"
  exit 0
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh is not installed (brew install gh)" >&2
  exit 1
fi

for entry in "${labels[@]}"; do
  IFS='|' read -r name color description <<<"$entry"
  gh label create "$name" --repo "$repo" --color "$color" --description "$description" --force
  echo "synced: $name"
done
echo "sync_labels: ${#labels[@]} labels synced to $repo"
