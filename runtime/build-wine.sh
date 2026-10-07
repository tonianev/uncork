#!/usr/bin/env bash
# build-wine.sh: build an x86_64, new-WoW64 Wine for macOS from CodeWeavers'
# CrossOver source tarball and package it as an Uncork runtime.
#
# EXPERIMENTAL. Uncork does not install what this produces yet; the runtimes
# it installs are the pinned upstream builds in runtime/catalog.toml. Run
# `runtime/build-wine.sh --help` for usage.
#
# The recipe follows dappermint/winecx-gptk's build workflow, which builds
# the same CrossOver tree on GitHub's arm64 macOS runners:
# https://github.com/dappermint/winecx-gptk/blob/main/.github/workflows/build.yml
#
#   1. fetch crossover-sources-<version>.tar.gz; verify size and SHA-256
#   2. extract sources/wine; apply runtime/patches/*.patch in name order
#   3. build Wine's build tools natively (make __tooldeps__)
#   4. configure the x86_64 build: clang -arch x86_64 for the Unix side,
#      mingw-w64 GCC for the PE side, libraries from --deps-prefix
#   5. make; make install-lib into a staging directory
#   6. make the tree relocatable: copy non-system dylibs into lib/, point
#      load commands at @loader_path, add an rpath, ad-hoc sign
#   7. gates: relocatable, deployment floor, i386 half, ntdll.so, media
#   8. package uncork-wine-<version>-x86_64.tar.xz with SOURCES.json
#
# Runs natively on an Apple Silicon Mac with Rosetta 2, or on an Intel Mac.
# Never under `arch -x86_64`, never as root. This file is written for the
# bash 3.2 that macOS ships: no associative arrays, no mapfile, and empty
# arrays are expanded as ${a[@]+"${a[@]}"} because of `set -u`.
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"

readonly DEFAULT_CROSSOVER_VERSION="26.3.0"
readonly SOURCE_URL_BASE="https://media.codeweavers.com/pub/crossover/source"
readonly REPOSITORY_URL="https://github.com/tonianev/uncork"
# The oldest macOS the runtime loads on. 15.0 is the floor for
# ROSETTA_ADVERTISE_AVX and DXMT's Metal 3.2 path.
readonly DEPLOYMENT_TARGET="15.0"
readonly HOST_TRIPLE="x86_64-apple-darwin24"
# Compiled-in prefix only. Wine finds its files relative to its own
# binaries, so the packaged tree works wherever it is unpacked.
readonly INSTALL_PREFIX="/opt/uncork-wine"
# A full i386 half is about a thousand PE modules. A build with far fewer
# fails every 32-bit program with c0000135 (STATUS_DLL_NOT_FOUND).
readonly MIN_I386_PE_FILES=500

# CrossOver source releases: "version|size in bytes|sha256". Sizes are the
# Content-Length the server reported on 2026-10-07.
#
# TODO(pin): no maintainer has hashed these tarballs yet, so every entry is
# TODO and the script refuses them unless --allow-unpinned is given. To pin
# one: download it yourself, run `shasum -a 256` on your copy, replace TODO
# in a reviewed pull request, and never copy a hash from somewhere else.
# For cross-checking your own result only: public build scripts (for example
# gauthierpiarrette/highball-engine's inputs.json) record 26.3.0 as
# ac99c8ca4b3848f3e81784135f023df266b61c2345726ea55a50b3e030dd6872.
CROSSOVER_PINS=(
  "26.3.0|149054023|TODO"
  "26.2.0|149047436|TODO"
  "26.1.0|149051164|TODO"
  "26.0.0|149088274|TODO"
)

# Options (see usage).
crossover_version="$DEFAULT_CROSSOVER_VERSION"
deps_prefixes=()
work_dir="$repo_root/runtime/work"
out_dir="$repo_root/runtime/out"
jobs=""
with_media=1
with_sdl=1
allow_unpinned=0
check_only=0
prepare_only=0
source_tarball=""

# State shared between steps.
tmp_dir=""
logs_dir=""
src_dir=""
tools_dir=""
build_dir=""
stage_dir=""
pkg_name=""
pkg_dir=""
libdir=""
pinned_size=""
pinned_sha256=""
source_pinned=false
source_path=""
source_sha256=""
source_size=""
configure_args=()
bundled_libs=()
patches=()

log() { printf '==> %s\n' "$*" >&2; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<EOF
Usage: runtime/build-wine.sh --deps-prefix DIR [options]

Build an x86_64, new-WoW64 Wine for macOS from CodeWeavers' CrossOver source
tarball, apply runtime/patches/*.patch, and package it as
OUT_DIR/uncork-wine-<version>-x86_64.tar.xz with a SOURCES.json manifest.
EXPERIMENTAL. A full build takes the better part of an hour.

Options:
  --crossover-version X.Y.Z  CrossOver source release (default: $DEFAULT_CROSSOVER_VERSION)
  --source-tarball FILE      use a crossover-sources tarball you already
                             downloaded instead of fetching it; it is
                             verified against the same pin
  --deps-prefix DIR          an x86_64 dependency prefix with lib/, include/
                             and lib/pkgconfig/; repeat for several. Together
                             they must provide freetype2, gnutls, sdl2,
                             libMoltenVK.dylib and, for media, gstreamer-1.0
                             and FFmpeg (libavcodec, libavformat, libavutil)
  --without-sdl              build without SDL2 (no SDL game controller backend)
  --without-media            build without GStreamer and FFmpeg (no video playback)
  --work-dir DIR             scratch space (default: runtime/work)
  --out-dir DIR              where the tarball goes (default: runtime/out)
  --jobs N                   parallel make jobs (default: logical CPUs)
  --allow-unpinned           build a source tarball that has no pinned SHA-256
                             in this script; never publish such a build
  --check-only               check prerequisites, then exit
  --prepare-only             fetch, verify and extract the source and apply
                             the patches into WORK_DIR, then exit (for
                             writing patches; needs no --deps-prefix)
  -h, --help                 show this help

Prerequisites (Homebrew names): Xcode Command Line Tools, Rosetta 2 on Apple
Silicon, mingw-w64 (x86_64 and i686 GCC), bison 3 or newer, flex, pkgconf,
and a native freetype for Wine's font tool. Paths must not contain spaces.

Outputs in OUT_DIR:
  uncork-wine-<version>-x86_64.tar.xz              the runtime
  uncork-wine-<version>-x86_64.tar.xz.sha256       its checksum
  uncork-wine-<version>-x86_64.SOURCES.json        what it was built from
Build logs are in WORK_DIR/logs.
EOF
}

# --- Arguments ----------------------------------------------------------------

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --*=*)
        # Split --opt=value into --opt value and look at it again.
        local opt="${1%%=*}" value="${1#*=}"
        shift
        set -- "$opt" "$value" "$@"
        continue
        ;;
    esac
    case "$1" in
      -h | --help)
        usage
        exit 0
        ;;
      --crossover-version)
        require_value "$@"
        crossover_version="$2"
        shift 2
        ;;
      --source-tarball)
        require_value "$@"
        source_tarball="$2"
        shift 2
        ;;
      --deps-prefix)
        require_value "$@"
        deps_prefixes+=("$2")
        shift 2
        ;;
      --work-dir)
        require_value "$@"
        work_dir="$2"
        shift 2
        ;;
      --out-dir)
        require_value "$@"
        out_dir="$2"
        shift 2
        ;;
      --jobs)
        require_value "$@"
        jobs="$2"
        shift 2
        ;;
      --without-sdl)
        with_sdl=0
        shift
        ;;
      --without-media)
        with_media=0
        shift
        ;;
      --allow-unpinned)
        allow_unpinned=1
        shift
        ;;
      --check-only)
        check_only=1
        shift
        ;;
      --prepare-only)
        prepare_only=1
        shift
        ;;
      *) die "unknown argument: $1 (see --help)" ;;
    esac
  done
}

# require_value OPTION [VALUE ...]: OPTION must be followed by a value.
require_value() {
  if [ $# -lt 2 ] || [ -z "$2" ]; then
    die "$1 needs a value (see --help)"
  fi
  case "$2" in
    -*) die "$1 needs a value, got option $2 (see --help)" ;;
  esac
}

# is_version STRING: a strict X.Y.Z, which is also safe inside a URL.
is_version() {
  local re='^[0-9]+\.[0-9]+\.[0-9]+$'
  [[ "$1" =~ $re ]]
}

is_sha256() {
  local re='^[0-9a-f]{64}$'
  [[ "$1" =~ $re ]]
}

# has_space STRING: Wine's makefiles break on whitespace in paths.
has_space() {
  case "$1" in
    *[[:space:]]*) return 0 ;;
    *) return 1 ;;
  esac
}

# absolute_dir DIR: create DIR if needed and print its physical path.
absolute_dir() {
  mkdir -p "$1"
  (cd "$1" && pwd -P)
}

validate_args() {
  is_version "$crossover_version" ||
    die "--crossover-version must look like 26.3.0, got '$crossover_version'"
  if [ -z "$jobs" ]; then
    jobs="$(sysctl -n hw.logicalcpu 2>/dev/null || echo 4)"
  fi
  case "$jobs" in
    '' | *[!0-9]* | 0) die "--jobs must be a positive integer, got '$jobs'" ;;
  esac
  if [ -n "$source_tarball" ] && [ ! -f "$source_tarball" ]; then
    die "--source-tarball $source_tarball is not a file"
  fi
  local dir
  for dir in "$work_dir" "$out_dir" ${deps_prefixes[@]+"${deps_prefixes[@]}"}; do
    if has_space "$dir"; then
      die "path '$dir' contains whitespace, which Wine's build system does not support"
    fi
  done
}

# --- Source pin -----------------------------------------------------------------

# pin_for VERSION: print "size|sha256" for VERSION, or fail if it is unknown.
pin_for() {
  local entry
  for entry in "${CROSSOVER_PINS[@]}"; do
    case "$entry" in
      "$1|"*)
        printf '%s\n' "${entry#*|}"
        return 0
        ;;
    esac
  done
  return 1
}

resolve_pin() {
  local pin=""
  pinned_size=""
  pinned_sha256=""
  if pin="$(pin_for "$crossover_version")"; then
    IFS='|' read -r pinned_size pinned_sha256 <<<"$pin"
  fi
  if is_sha256 "$pinned_sha256"; then
    source_pinned=true
    return 0
  fi
  case "$pinned_sha256" in
    '' | TODO) ;;
    *) die "the pin for $crossover_version is malformed: '$pinned_sha256' (bug in this script)" ;;
  esac
  source_pinned=false
  pinned_sha256=""
  if [ "$allow_unpinned" -ne 1 ]; then
    die "crossover-sources-$crossover_version.tar.gz has no pinned SHA-256 in runtime/build-wine.sh.
Download it yourself, hash it with 'shasum -a 256', and pin it in CROSSOVER_PINS
in a reviewed pull request. To build it anyway (never publish the result),
rerun with --allow-unpinned; the script then prints the hash it saw."
  fi
  warn "crossover-sources-$crossover_version.tar.gz is not pinned; building it because of --allow-unpinned"
}

# --- Prerequisites --------------------------------------------------------------

problems=()
problem() { problems+=("$*"); }

have() { command -v "$1" >/dev/null 2>&1; }

require_tool() {
  have "$1" || problem "$1 not found: $2"
}

# major_version STRING: the leading integer of the last word, "3" for
# "bison (GNU Bison) 3.8.2".
major_version() {
  local last="${1##* }"
  printf '%s\n' "${last%%.*}"
}

# deps_pkg_config ARGS...: pkg-config that sees only the --deps-prefix dirs.
deps_pkg_config() {
  PKG_CONFIG_LIBDIR="$(deps_pkgconfig_path)" PKG_CONFIG_PATH="" pkg-config "$@"
}

deps_pkgconfig_path() {
  local path="" prefix dir
  for prefix in ${deps_prefixes[@]+"${deps_prefixes[@]}"}; do
    for dir in "$prefix/lib/pkgconfig" "$prefix/share/pkgconfig"; do
      if [ -d "$dir" ]; then
        path="${path:+$path:}$dir"
      fi
    done
  done
  printf '%s\n' "$path"
}

# find_in_deps FILE_NAME: print the first lib/FILE_NAME in the deps prefixes.
find_in_deps() {
  local prefix
  for prefix in ${deps_prefixes[@]+"${deps_prefixes[@]}"}; do
    if [ -e "$prefix/lib/$1" ]; then
      printf '%s\n' "$prefix/lib/$1"
      return 0
    fi
  done
  return 1
}

# has_x86_64 FILE: FILE is a Mach-O with an x86_64 slice.
has_x86_64() {
  local archs
  archs="$(lipo -archs "$1" 2>/dev/null)" || return 1
  case " $archs " in
    *" x86_64 "*) return 0 ;;
    *) return 1 ;;
  esac
}

required_deps_modules() {
  printf '%s\n' freetype2 gnutls
  if [ "$with_sdl" -eq 1 ]; then
    printf '%s\n' sdl2
  fi
  if [ "$with_media" -eq 1 ]; then
    printf '%s\n' gstreamer-1.0 gstreamer-video-1.0 gstreamer-audio-1.0 \
      libavcodec libavformat libavutil
  fi
}

check_host() {
  [ "$(uname -s)" = Darwin ] || die "this script builds Wine for macOS and must run on macOS"
  [ "$(id -u)" -ne 0 ] || die "do not run this script as root or with sudo"
  if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    die "this shell runs under Rosetta (arch -x86_64?). Run the script natively; Wine's build tools are built for the host and only the Wine build itself targets x86_64"
  fi
  if [ "$(uname -m)" = arm64 ] && ! /usr/bin/arch -x86_64 /usr/bin/true 2>/dev/null; then
    problem "Rosetta 2 is not installed: softwareupdate --install-rosetta --agree-to-license"
  fi
}

check_toolchain() {
  local tool version missing=""
  require_tool xcrun "install the Xcode Command Line Tools: xcode-select --install"
  for tool in clang make curl tar patch otool install_name_tool codesign lipo plutil file; do
    require_tool "$tool" "install the Xcode Command Line Tools: xcode-select --install"
  done
  have shasum || have openssl || problem "neither shasum nor openssl found to compute SHA-256"
  for tool in x86_64-w64-mingw32-gcc i686-w64-mingw32-gcc x86_64-w64-mingw32-strip x86_64-w64-mingw32-objdump; do
    have "$tool" || missing="$missing $tool"
  done
  if [ -n "$missing" ]; then
    problem "mingw-w64 tools not found:$missing. brew install mingw-w64 (GCC, not llvm-mingw: an llvm-mingw kernelbase.dll breaks the Steam login)"
  fi
  require_tool flex "brew install flex"
  require_tool pkg-config "brew install pkgconf"
  if have bison; then
    version="$(bison --version 2>/dev/null | sed -n 1p)"
    if [ "$(major_version "$version")" -lt 3 ] 2>/dev/null; then
      problem "bison 3 or newer is needed, found '$version' at $(command -v bison): brew install bison, then put \$(brew --prefix bison)/bin first on PATH"
    fi
  else
    problem "bison not found: brew install bison, then put \$(brew --prefix bison)/bin first on PATH"
  fi
  if have xcrun && ! xcrun --show-sdk-path >/dev/null 2>&1; then
    problem "no macOS SDK found: xcode-select --install"
  fi
  if have clang && ! probe_x86_64_link; then
    problem "clang cannot link an x86_64 program against the macOS SDK"
  fi
  if have pkg-config && ! pkg-config --exists freetype2 2>/dev/null; then
    problem "no native freetype2 for Wine's font tool (sfnt2fon): brew install freetype"
  fi
}

probe_x86_64_link() {
  printf 'int main(void) { return 0; }\n' >"$tmp_dir/probe.c"
  clang -arch x86_64 "$tmp_dir/probe.c" -o "$tmp_dir/probe" >/dev/null 2>&1
}

check_deps() {
  local prefix module moltenvk lib_dir
  if [ "${#deps_prefixes[@]}" -eq 0 ]; then
    problem "no --deps-prefix given: Wine's x86_64 half needs x86_64 builds of $(required_deps_modules | tr '\n' ' ')and libMoltenVK.dylib"
    return 0
  fi
  for prefix in "${deps_prefixes[@]}"; do
    case "$prefix" in
      /*) ;;
      *) problem "--deps-prefix $prefix must be an absolute path" ;;
    esac
    [ -d "$prefix" ] || problem "--deps-prefix $prefix is not a directory"
  done
  have pkg-config || return 0
  if [ -z "$(deps_pkgconfig_path)" ]; then
    problem "no --deps-prefix has a lib/pkgconfig or share/pkgconfig directory"
  else
    for module in $(required_deps_modules); do
      deps_pkg_config --exists "$module" 2>/dev/null ||
        problem "pkg-config module $module not found in any --deps-prefix"
    done
  fi
  if deps_pkg_config --exists freetype2 2>/dev/null; then
    lib_dir="$(deps_pkg_config --variable=libdir freetype2)"
    if [ -e "$lib_dir/libfreetype.dylib" ] && ! has_x86_64 "$lib_dir/libfreetype.dylib"; then
      problem "$lib_dir/libfreetype.dylib has no x86_64 slice; --deps-prefix must hold x86_64 libraries"
    fi
  fi
  if moltenvk="$(find_in_deps libMoltenVK.dylib)"; then
    has_x86_64 "$moltenvk" || problem "$moltenvk has no x86_64 slice (use Khronos' MoltenVK-macos.tar release)"
  else
    problem "libMoltenVK.dylib not found in any --deps-prefix/lib (Khronos MoltenVK-macos.tar, MoltenVK/dynamic/dylib/macOS/libMoltenVK.dylib)"
  fi
}

check_prerequisites() {
  log "checking prerequisites"
  problems=()
  check_host
  check_toolchain
  check_deps
  if [ "${#problems[@]}" -gt 0 ]; then
    printf 'error: prerequisites missing:\n' >&2
    printf '  - %s\n' "${problems[@]}" >&2
    exit 1
  fi
  SDKROOT="$(xcrun --show-sdk-path)"
  export SDKROOT
  log "prerequisites: ok (SDK $(xcrun --show-sdk-version), $jobs jobs)"
}

# --- Small helpers ----------------------------------------------------------------

sha256_of() {
  local out
  if have shasum; then
    out="$(shasum -a 256 "$1")"
  else
    out="$(openssl dgst -sha256 -r "$1")"
  fi
  printf '%s\n' "${out%% *}"
}

size_of() {
  local size
  size="$(wc -c <"$1")"
  printf '%s\n' "${size//[[:space:]]/}"
}

# run_logged NAME COMMAND...: run COMMAND with its output in logs/NAME.log;
# on failure print the end of the log and stop.
run_logged() {
  local name="$1" log_file="$logs_dir/$1.log"
  shift
  log "$name (log: $log_file)"
  if ! "$@" >"$log_file" 2>&1; then
    printf '\n--- last 60 lines of %s ---\n' "$log_file" >&2
    tail -n 60 "$log_file" >&2
    die "$name failed; full log: $log_file"
  fi
}

# relpath FROM_DIR TO_DIR: relative path between two absolute, normalized
# directories, "." when they are the same.
relpath() {
  local from="$1" to="$2" up="" rest=""
  while [ "$to" != "$from" ] && [ "${to#"$from"/}" = "$to" ]; do
    [ "$from" != / ] || die "relpath: $2 is not reachable from $1"
    from="$(dirname "$from")"
    up="../$up"
  done
  if [ "$to" != "$from" ]; then
    rest="${to#"$from"/}"
  fi
  rest="$up$rest"
  rest="${rest%/}"
  printf '%s\n' "${rest:-.}"
}

# version_code X[.Y[.Z]]: a comparable integer, 150000 for 15.0.
version_code() {
  local major minor patch
  IFS=. read -r major minor patch <<<"$1"
  printf '%d\n' $((10#${major:-0} * 10000 + 10#${minor:-0} * 100 + 10#${patch:-0}))
}

json_str() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  s="${s//$'\t'/\\t}"
  printf '"%s"' "$s"
}

# json_array VALUE...: a JSON array of strings on one line.
json_array() {
  local out="" value
  for value in "$@"; do
    out="${out:+$out, }$(json_str "$value")"
  done
  printf '[%s]' "$out"
}

# --- Mach-O inspection ------------------------------------------------------------
#
# awk never exits early here: under `set -o pipefail` an early exit kills
# otool with SIGPIPE and the whole pipeline fails.

# load_commands FILE: every dylib FILE loads, one per line.
load_commands() {
  otool -l "$1" | awk '
    $1 == "cmd" { want = ($2 ~ /^LC_(LOAD|LOAD_WEAK|REEXPORT|LAZY_LOAD|LOAD_UPWARD)_DYLIB$/) }
    want && $1 == "name" { print $2; want = 0 }
  ' | sort -u
}

# install_id FILE: the install name of a dylib, empty for other Mach-O files.
install_id() {
  otool -D "$1" | sed -n 2p
}

rpaths_of() {
  otool -l "$1" | awk '
    $1 == "cmd" { want = ($2 == "LC_RPATH") }
    want && $1 == "path" { print $2; want = 0 }
  '
}

# minos_of FILE: the minimum macOS version FILE was linked for.
minos_of() {
  otool -l "$1" | awk '
    $1 == "cmd" { build = ($2 == "LC_BUILD_VERSION"); legacy = ($2 == "LC_VERSION_MIN_MACOSX") }
    !found && build && $1 == "minos" { print $2; found = 1 }
    !found && legacy && $1 == "version" { print $2; found = 1 }
  '
}

is_system_path() {
  case "$1" in
    /usr/lib/* | /System/*) return 0 ;;
    *) return 1 ;;
  esac
}

is_macho() {
  local kind
  kind="$(file -b "$1")"
  case "$kind" in
    *Mach-O*) return 0 ;;
    *) return 1 ;;
  esac
}

# macho_files DIR: regular Mach-O files under DIR (symlinks skipped), sorted.
# The PE half (lib/wine/*-windows) is skipped; it holds no Mach-O files.
macho_files() {
  local f
  find "$1" -type f ! -path '*/lib/wine/*-windows/*' -print | sort | while IFS= read -r f; do
    if is_macho "$f"; then
      printf '%s\n' "$f"
    fi
  done
}

# --- Steps ------------------------------------------------------------------------

prepare_dirs() {
  work_dir="$(absolute_dir "$work_dir")"
  out_dir="$(absolute_dir "$out_dir")"
  logs_dir="$work_dir/logs"
  mkdir -p "$logs_dir" "$work_dir/downloads"
  tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/uncork-build-wine.XXXXXX")"
  trap 'rm -rf "$tmp_dir"' EXIT
}

# verify_tarball FILE: check FILE against the pin and record its hash.
# Prints why and fails on a mismatch; prints the hash when unpinned.
verify_tarball() {
  local file="$1" size sha
  size="$(size_of "$file")"
  if [ -n "$pinned_size" ] && [ "$size" != "$pinned_size" ]; then
    printf 'error: %s is %s bytes; the pin for %s says %s\n' "$file" "$size" "$crossover_version" "$pinned_size" >&2
    return 1
  fi
  sha="$(sha256_of "$file")"
  if [ "$source_pinned" = true ]; then
    if [ "$sha" != "$pinned_sha256" ]; then
      printf 'error: %s has SHA-256 %s; the pin for %s says %s\n' "$file" "$sha" "$crossover_version" "$pinned_sha256" >&2
      return 1
    fi
    log "source verified: sha256 $sha, $size bytes"
  else
    warn "unpinned source: sha256 $sha, $size bytes. Pin it only after hashing your own download."
  fi
  source_sha256="$sha"
  source_size="$size"
}

# fetch_source: set source_path to a verified crossover-sources tarball.
fetch_source() {
  local name="crossover-sources-$crossover_version.tar.gz"
  local url="$SOURCE_URL_BASE/$name"
  local dest="$work_dir/downloads/$name"
  if [ -n "$source_tarball" ]; then
    log "using $source_tarball"
    verify_tarball "$source_tarball" || die "refusing $source_tarball"
    source_path="$source_tarball"
    return 0
  fi
  if [ -f "$dest" ]; then
    log "reusing $dest"
    if verify_tarball "$dest"; then
      source_path="$dest"
      return 0
    fi
    warn "discarding the cached download"
    rm -f "$dest"
  fi
  log "downloading $url"
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --retry 3 --connect-timeout 30 --silent --show-error \
    --output "$dest.part" "$url"
  verify_tarball "$dest.part" || die "refusing the download from $url"
  mv "$dest.part" "$dest"
  source_path="$dest"
}

extract_source() {
  local src_root="$work_dir/src-$crossover_version"
  rm -rf "$src_root"
  mkdir -p "$src_root"
  log "extracting sources/wine"
  tar -xzf "$source_path" -C "$src_root" sources/wine ||
    die "could not extract sources/wine from $source_path"
  src_dir="$src_root/sources/wine"
  [ -x "$src_dir/configure" ] || die "$src_dir/configure is missing; the tarball layout changed"
  [ -f "$src_dir/COPYING.LIB" ] || die "$src_dir/COPYING.LIB is missing; the tarball layout changed"
  log "source: $(sed -n 1p "$src_dir/VERSION" 2>/dev/null || echo 'unknown Wine version')"
}

collect_patches() {
  local p
  patches=()
  for p in "$script_dir"/patches/*.patch; do
    if [ -f "$p" ]; then
      patches+=("$p")
    fi
  done
}

apply_patches() {
  local p
  collect_patches
  if [ "${#patches[@]}" -eq 0 ]; then
    log "no patches in runtime/patches"
    return 0
  fi
  for p in "${patches[@]}"; do
    # -f: never prompt and never guess a reversed patch; -F 0: no fuzz. A
    # patch that does not apply exactly stops the build.
    patch -p1 -f -F 0 -s --dry-run -d "$src_dir" -i "$p" >/dev/null ||
      die "$(basename "$p") does not apply cleanly to crossover-sources-$crossover_version"
    patch -p1 -f -F 0 -s -d "$src_dir" -i "$p"
    log "applied $(basename "$p")"
  done
}

# mingw_env: force mingw-w64 GCC for both PE architectures. Wine's configure
# would also take llvm-mingw or plain clang if those come first on PATH.
mingw_env() {
  export i386_CC=i686-w64-mingw32-gcc
  export x86_64_CC=x86_64-w64-mingw32-gcc
  export CROSSCFLAGS="-O2"
}

configure_tools() {
  mingw_env
  "$src_dir/configure" \
    --enable-archs=i386,x86_64 \
    --disable-tests \
    --with-freetype \
    --without-x --without-wayland --without-gstreamer --without-ffmpeg \
    --without-oss --without-alsa --without-pulse --without-sane \
    --without-usb --without-v4l2 --without-pcap --without-capi \
    --without-opencl --without-cups
}

build_tools() {
  tools_dir="$work_dir/build-tools-$crossover_version"
  rm -rf "$tools_dir"
  mkdir -p "$tools_dir"
  # Built for the host so the thousands of winebuild/widl/wrc runs are not
  # Rosetta processes. Cross-configuring needs --with-wine-tools anyway.
  (cd "$tools_dir" && run_logged tools-configure configure_tools)
  (cd "$tools_dir" && run_logged tools-make make -j"$jobs" __tooldeps__)
  # wrc reads locale.nls relative to its own binary (tools/wrc/../../nls),
  # and __tooldeps__ never populates an out-of-tree nls/ directory. Link the
  # file, not the directory, so an existing nls/ cannot swallow the link.
  rm -rf "$tools_dir/nls"
  mkdir "$tools_dir/nls"
  ln -s "$src_dir/nls/locale.nls" "$tools_dir/nls/locale.nls"
  [ -r "$tools_dir/nls/locale.nls" ] || die "$tools_dir/nls/locale.nls does not resolve"
}

# soname_override VAR LIB: make Wine dlopen libLIB by the leaf of its install
# name (libfreetype.6.dylib) so the bundled copy in lib/ is found.
soname_override() {
  local var="$1" lib="$2" path id
  path="$(find_in_deps "lib$lib.dylib")" || return 0
  id="$(install_id "$path")"
  [ -n "$id" ] || die "$path has no install name"
  export "ac_cv_lib_soname_$var=${id##*/}"
}

configure_wine_env() {
  local prefix cflags="" ldflags=""
  for prefix in "${deps_prefixes[@]}"; do
    if [ -d "$prefix/include" ]; then
      cflags="$cflags -I$prefix/include"
    fi
    if [ -d "$prefix/lib" ]; then
      ldflags="$ldflags -L$prefix/lib"
    fi
  done
  mingw_env
  export CC="clang -arch x86_64"
  export CXX="clang++ -arch x86_64"
  # -Werror=unguarded-availability-new turns a call to an API above the
  # deployment floor into a build error instead of a weak link that
  # crashes on the older OS. Host flags only; mingw does not know it.
  export CFLAGS="-O2 -Wno-error=implicit-function-declaration -Werror=unguarded-availability-new$cflags"
  export LDFLAGS="-Wl,-headerpad_max_install_names$ldflags"
  # Only the deps prefixes: a Homebrew arm64 library must never satisfy an
  # x86_64 check.
  PKG_CONFIG_LIBDIR="$(deps_pkgconfig_path)"
  export PKG_CONFIG_LIBDIR
  export PKG_CONFIG_PATH=""
  # The macOS 27 SDK declares pipe2 and dup3 as macOS 27.0 APIs. Wine falls
  # back to pipe + fcntl without pipe2, so refuse the detection and keep the
  # 15.0 floor; dup3 is refused the same way in case configure probes it.
  export ac_cv_func_pipe2=no
  export ac_cv_func_dup3=no
  # MoltenVK is loaded directly. An empty, set cache value stops configure
  # from preferring a vulkan-loader it might find in a deps prefix.
  export ac_cv_lib_soname_vulkan=""
  soname_override freetype freetype
  soname_override gnutls gnutls
  soname_override MoltenVK MoltenVK
  if [ "$with_sdl" -eq 1 ]; then
    soname_override SDL2 SDL2
  fi
}

configure_wine() {
  build_dir="$work_dir/build-$crossover_version"
  rm -rf "$build_dir"
  mkdir -p "$build_dir"
  configure_args=(
    --host="$HOST_TRIPLE"
    --with-wine-tools="$tools_dir"
    "--enable-archs=i386,x86_64"
    --prefix="$INSTALL_PREFIX"
    --disable-tests
    --without-x --without-wayland
    --with-vulkan --with-coreaudio --with-gnutls --with-freetype
  )
  if [ "$with_sdl" -eq 1 ]; then
    configure_args+=(--with-sdl)
  else
    configure_args+=(--without-sdl)
  fi
  if [ "$with_media" -eq 1 ]; then
    configure_args+=(--with-gstreamer --with-ffmpeg)
  else
    configure_args+=(--without-gstreamer --without-ffmpeg)
  fi
  (
    cd "$build_dir"
    configure_wine_env
    run_logged configure "$src_dir/configure" "${configure_args[@]}"
  )
}

# gate_configuration: what configure only warns about, refuse here.
gate_configuration() {
  local config_h="$build_dir/include/config.h" line value
  [ -f "$config_h" ] || die "$config_h is missing after configure"
  while IFS= read -r line; do
    value="${line#*\"}"
    value="${value%\"*}"
    case "$value" in
      */*) die "Wine would dlopen a path, not a library name: '$line'. Set the matching ac_cv_lib_soname_* to a leaf name" ;;
    esac
  done < <(grep '^#define SONAME_' "$config_h" || true)
  if [ "$with_media" -eq 1 ]; then
    grep -q '^#define HAVE_FFMPEG 1' "$config_h" ||
      die "FFmpeg was not found: winedmo would build with no decoder backend"
    grep -Eq '^GSTREAMER_LIBS *= *[^ ]' "$build_dir/Makefile" ||
      die "GStreamer was not found: winegstreamer would not be built"
  fi
  log "configuration: ok"
}

build_wine() {
  (cd "$build_dir" && run_logged make make -j"$jobs")
}

install_wine() {
  stage_dir="$work_dir/stage-$crossover_version"
  rm -rf "$stage_dir"
  # install-lib, not install: install also builds and installs some two
  # thousand import libraries the runtime never uses.
  (cd "$build_dir" && run_logged install make -j"$jobs" install-lib DESTDIR="$stage_dir")
  [ -d "$stage_dir$INSTALL_PREFIX/lib/wine" ] || die "install-lib produced no lib/wine"
}

assemble_package() {
  pkg_name="uncork-wine-$crossover_version-x86_64"
  pkg_dir="$work_dir/package/$pkg_name"
  rm -rf "$work_dir/package"
  mkdir -p "$pkg_dir"
  cp -pR "$stage_dir$INSTALL_PREFIX/." "$pkg_dir/"
  libdir="$pkg_dir/lib"
  find "$pkg_dir/lib/wine" -name '*.a' -type f -delete

  # Wine 11 installs the loader beside ntdll.so in lib/wine/x86_64-unix and
  # leaves bin/ with wineserver only. The loader looks for ntdll.so in the
  # directory of its own realpath, so bin/wine must be a symlink: a copy
  # would look for bin/ntdll.so.
  if [ -x "$pkg_dir/lib/wine/x86_64-unix/wine" ]; then
    mkdir -p "$pkg_dir/bin"
    rm -f "$pkg_dir/bin/wine"
    ln -s ../lib/wine/x86_64-unix/wine "$pkg_dir/bin/wine"
  fi

  # LGPL-2.1 section 6: the license text, and what the build was made from.
  local doc_dir="$pkg_dir/share/doc/uncork-wine" f
  mkdir -p "$doc_dir/patches"
  for f in COPYING.LIB LICENSE AUTHORS VERSION; do
    if [ -f "$src_dir/$f" ]; then
      cp "$src_dir/$f" "$doc_dir/$f"
    fi
  done
  cp "$script_dir/build-wine.sh" "$doc_dir/build-wine.sh"
  for f in ${patches[@]+"${patches[@]}"}; do
    cp "$f" "$doc_dir/patches/"
  done
}

strip_pe_debug_info() {
  log "stripping DWARF from the PE half"
  # One strip per batch of files; strip complains about the few non-COFF
  # files (.nls, .tlb) and carries on with the rest of the batch.
  find "$pkg_dir/lib/wine" -path '*-windows/*' -type f -print0 |
    xargs -0 -n 64 -P "$jobs" x86_64-w64-mingw32-strip --strip-debug 2>/dev/null || true
  local arch headers
  for arch in i386 x86_64; do
    headers="$(x86_64-w64-mingw32-objdump -h "$pkg_dir/lib/wine/$arch-windows/ntdll.dll")"
    if grep -q '\.debug_info' <<<"$headers"; then
      die "lib/wine/$arch-windows/ntdll.dll still carries DWARF; the strip step failed"
    fi
  done
}

# foreign_deps FILE: dependencies of FILE that must be copied into lib/:
# absolute non-system paths, and @rpath/@loader_path names that only a deps
# prefix provides.
foreign_deps() {
  local file="$1" dep base found deps
  deps="$(load_commands "$file")"
  while IFS= read -r dep; do
    case "$dep" in
      '') ;;
      /usr/lib/* | /System/*) ;;
      /*)
        [ -e "$dep" ] || die "$file links $dep, which does not exist"
        printf '%s\n' "$dep"
        ;;
      @rpath/* | @loader_path/* | @executable_path/*)
        base="${dep##*/}"
        if [ ! -e "$libdir/$base" ] && found="$(find_in_deps "$base")"; then
          printf '%s\n' "$found"
        fi
        ;;
    esac
  done <<<"$deps"
}

# thin_to_x86_64 FILE: keep only the x86_64 slice; nothing else here loads.
thin_to_x86_64() {
  local archs
  archs="$(lipo -archs "$1")"
  case "$archs" in
    x86_64) ;;
    *x86_64*)
      lipo -thin x86_64 "$1" -output "$1.thin"
      mv "$1.thin" "$1"
      ;;
    *) die "$1 has no x86_64 slice (archs: $archs)" ;;
  esac
}

# bundle_closure LIB...: copy LIB and everything it links into lib/.
bundle_closure() {
  local queue=("$@") next lib base deps dep
  while [ "${#queue[@]}" -gt 0 ]; do
    next=()
    for lib in "${queue[@]}"; do
      base="${lib##*/}"
      [ ! -e "$libdir/$base" ] || continue
      cp -L "$lib" "$libdir/$base"
      chmod u+w "$libdir/$base"
      thin_to_x86_64 "$libdir/$base"
      bundled_libs+=("$base")
      deps="$(foreign_deps "$libdir/$base")"
      while IFS= read -r dep; do
        if [ -n "$dep" ]; then
          next+=("$dep")
        fi
      done <<<"$deps"
    done
    queue=(${next[@]+"${next[@]}"})
  done
}

# bundle_gstreamer_plugins: GStreamer dlopens its plugins from a directory,
# so no load command names them. The launcher must point
# GST_PLUGIN_SYSTEM_PATH_1_0 at lib/gstreamer-1.0.
bundle_gstreamer_plugins() {
  local prefix plugin dest deps
  for prefix in "${deps_prefixes[@]}"; do
    [ -d "$prefix/lib/gstreamer-1.0" ] || continue
    for plugin in "$prefix"/lib/gstreamer-1.0/*.dylib; do
      [ -f "$plugin" ] || continue
      dest="$libdir/gstreamer-1.0/${plugin##*/}"
      [ ! -e "$dest" ] || continue
      mkdir -p "$libdir/gstreamer-1.0"
      cp -L "$plugin" "$dest"
      chmod u+w "$dest"
      thin_to_x86_64 "$dest"
      deps="$(foreign_deps "$dest")"
      if [ -n "$deps" ]; then
        # shellcheck disable=SC2086 # one path per line, no spaces (checked)
        bundle_closure $deps
      fi
    done
  done
}

# fixup FILE: rewrite FILE's install name and dependencies on bundled
# libraries to @loader_path, so the tree loads wherever it is unpacked.
fixup() {
  local file="$1" dir rel id dep base target deps changes=()
  dir="$(cd "$(dirname "$file")" && pwd -P)"
  rel="$(relpath "$dir" "$libdir")"
  if [ "$rel" = . ]; then
    target="@loader_path"
  else
    target="@loader_path/$rel"
  fi
  id="$(install_id "$file")"
  case "$id" in
    /*) is_system_path "$id" || changes+=(-id "@loader_path/${file##*/}") ;;
  esac
  deps="$(load_commands "$file")"
  while IFS= read -r dep; do
    base="${dep##*/}"
    case "$dep" in
      '' | /usr/lib/* | /System/*) continue ;;
      /* | @rpath/* | @loader_path/* | @executable_path/*) ;;
      *) continue ;;
    esac
    [ -e "$libdir/$base" ] || continue
    [ "$dep" != "$target/$base" ] || continue
    changes+=(-change "$dep" "$target/$base")
  done <<<"$deps"
  if [ "${#changes[@]}" -gt 0 ]; then
    quietly install_name_tool "${changes[@]}" "$file"
  fi
}

# add_rpath FILE PATH: add an LC_RPATH unless FILE already has it.
add_rpath() {
  local existing
  existing="$(rpaths_of "$1")"
  if ! grep -qxF "$2" <<<"$existing"; then
    quietly install_name_tool -add_rpath "$2" "$1"
  fi
}

# quietly COMMAND...: hide COMMAND's chatter (install_name_tool warns about
# every signature it invalidates), but show it and stop if COMMAND fails.
quietly() {
  local output
  if ! output="$("$@" 2>&1)"; then
    printf '%s\n' "$output" >&2
    die "failed: $*"
  fi
}

relocate() {
  log "bundling non-system libraries into lib/"
  local line soname path file deps files
  libdir="$(cd "$libdir" && pwd -P)"
  bundled_libs=()

  # 1. What Wine dlopens by name (config.h SONAME_*): freetype, gnutls,
  # MoltenVK, SDL2. A name no deps prefix has is left to the system.
  while IFS= read -r line; do
    soname="${line#*\"}"
    soname="${soname%\"*}"
    if path="$(find_in_deps "$soname")"; then
      bundle_closure "$path"
    else
      log "not bundled: $soname (expected from the system)"
    fi
  done < <(grep '^#define SONAME_' "$build_dir/include/config.h" || true)

  # 2. What Wine's own Mach-O files link directly (FFmpeg, GStreamer, ...).
  files="$(macho_files "$pkg_dir")"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    deps="$(foreign_deps "$file")"
    if [ -n "$deps" ]; then
      # shellcheck disable=SC2086 # one path per line, no spaces (checked)
      bundle_closure $deps
    fi
  done <<<"$files"

  if [ "$with_media" -eq 1 ]; then
    bundle_gstreamer_plugins
  fi

  log "rewriting load commands to @loader_path"
  files="$(macho_files "$pkg_dir")"
  while IFS= read -r file; do
    [ -z "$file" ] || fixup "$file"
  done <<<"$files"

  # The Unix modules dlopen libraries by bare name; dlopen searches the
  # caller's rpaths, and lib/ is two levels above lib/wine/x86_64-unix.
  for file in "$pkg_dir"/lib/wine/x86_64-unix/*.so; do
    [ ! -f "$file" ] || add_rpath "$file" "@loader_path/../../"
  done

  # install_name_tool invalidated the signatures. Sign everything ad hoc
  # last; x86_64 code may run unsigned under Rosetta, but not with a broken
  # signature.
  log "ad-hoc signing"
  while IFS= read -r file; do
    [ -z "$file" ] || quietly codesign --force --sign - "$file"
  done <<<"$files"
  if [ "${#bundled_libs[@]}" -gt 0 ]; then
    log "bundled ${#bundled_libs[@]} libraries: ${bundled_libs[*]}"
  fi
}

# --- Gates ------------------------------------------------------------------------

# gate_relocatable DIR: no Mach-O file names an absolute non-system path.
gate_relocatable() {
  local dir="$1" file id dep deps files bad=()
  files="$(macho_files "$dir")"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    id="$(install_id "$file")"
    case "$id" in
      /*) is_system_path "$id" || bad+=("id ${file#"$dir"/} -> $id") ;;
    esac
    deps="$(load_commands "$file")"
    while IFS= read -r dep; do
      case "$dep" in
        /*) is_system_path "$dep" || bad+=("dep ${file#"$dir"/} -> $dep") ;;
      esac
    done <<<"$deps"
  done <<<"$files"
  if [ "${#bad[@]}" -gt 0 ]; then
    printf '  %s\n' "${bad[@]}" >&2
    die "${#bad[@]} absolute non-system load command(s): the runtime would only work on this machine"
  fi
  log "relocatable: ok"
}

# gate_deployment_target DIR: nothing was linked for a newer macOS than the floor.
gate_deployment_target() {
  local dir="$1" floor file minos files bad=()
  floor="$(version_code "$DEPLOYMENT_TARGET")"
  files="$(macho_files "$dir")"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    minos="$(minos_of "$file")"
    if [ -n "$minos" ] && [ "$(version_code "$minos")" -gt "$floor" ]; then
      bad+=("${file#"$dir"/} (minos $minos)")
    fi
  done <<<"$files"
  if [ "${#bad[@]}" -gt 0 ]; then
    printf '  %s\n' "${bad[@]}" >&2
    die "${#bad[@]} file(s) need a macOS newer than $DEPLOYMENT_TARGET"
  fi
  log "deployment target: nothing newer than macOS $DEPLOYMENT_TARGET"
}

# gate_tree DIR: the files a WoW64 runtime cannot work without.
gate_tree() {
  local dir="$1" count path
  for path in bin/wine bin/wineserver lib/wine/x86_64-unix/ntdll.so \
    lib/wine/x86_64-windows/ntdll.dll lib/wine/i386-windows/ntdll.dll; do
    [ -e "$dir/$path" ] || die "$path is missing from the runtime"
  done
  [ -x "$dir/bin/wine" ] || die "bin/wine is not executable"
  count="$(find "$dir/lib/wine/i386-windows" -type f | wc -l)"
  count="${count//[[:space:]]/}"
  [ "$count" -ge "$MIN_I386_PE_FILES" ] ||
    die "lib/wine/i386-windows has $count files (expected at least $MIN_I386_PE_FILES); 32-bit programs would fail with c0000135"
  log "tree: ok ($count i386 PE files)"
}

# gate_media DIR: winedmo really links FFmpeg and winegstreamer exists.
gate_media() {
  local dir="$1" deps
  [ "$with_media" -eq 1 ] || return 0
  deps="$(load_commands "$dir/lib/wine/x86_64-unix/winedmo.so")"
  grep -q 'libavcodec' <<<"$deps" || die "winedmo.so does not link FFmpeg; video decoding would be dead"
  [ -f "$dir/lib/wine/x86_64-unix/winegstreamer.so" ] ||
    die "winegstreamer.so is missing; quartz, wmvcore and the mfplat decoders would be dead"
  log "media: ok"
}

# gate_links DIR: symlinks stay inside DIR; no hard links (Uncork's
# extractor rejects them, so they are turned into copies).
gate_links() {
  local dir="$1" link target resolved_dir file
  dir="$(cd "$dir" && pwd -P)"
  while IFS= read -r -d '' link; do
    target="$(readlink "$link")"
    case "$target" in
      /*) die "${link#"$dir"/} is an absolute symlink to $target" ;;
    esac
    resolved_dir="$(cd "$(dirname "$link")/$(dirname "$target")" 2>/dev/null && pwd -P)" ||
      die "${link#"$dir"/} -> $target does not resolve"
    case "$resolved_dir/" in
      "$dir"/*) ;;
      *) die "${link#"$dir"/} -> $target points outside the runtime" ;;
    esac
    [ -e "$link" ] || die "${link#"$dir"/} -> $target is dangling"
  done < <(find "$dir" -type l -print0)
  while IFS= read -r -d '' file; do
    cp -p "$file" "$file.unlinked"
    mv "$file.unlinked" "$file"
  done < <(find "$dir" -type f -links +1 -print0)
  log "links: ok"
}

# report_nx_compat DIR: count PE modules without NX_COMPAT. Without it a
# single module turns DEP off for the whole process, which makes Rosetta's
# page faults far more expensive. A warning for now, not a gate.
report_nx_compat() {
  local dir="$1" file headers flags total=0 missing=0 examples=""
  while IFS= read -r -d '' file; do
    headers="$(x86_64-w64-mingw32-objdump -p "$file" 2>/dev/null)" || continue
    flags="$(awk '$1 == "DllCharacteristics" && !seen { print $2; seen = 1 }' <<<"$headers")"
    [ -n "$flags" ] || continue
    total=$((total + 1))
    if (((16#$flags & 0x100) == 0)); then
      missing=$((missing + 1))
      if [ "$missing" -le 5 ]; then
        examples="$examples ${file#"$dir"/}"
      fi
    fi
  done < <(find "$dir/lib/wine" -path '*-windows/*' -type f \( -name '*.dll' -o -name '*.exe' \) -print0)
  if [ "$missing" -gt 0 ]; then
    warn "$missing of $total PE modules lack NX_COMPAT, for example:$examples"
  else
    log "NX_COMPAT: all $total PE modules"
  fi
}

run_gates() {
  gate_tree "$pkg_dir"
  gate_links "$pkg_dir"
  gate_relocatable "$pkg_dir"
  gate_deployment_target "$pkg_dir"
  gate_media "$pkg_dir"
  report_nx_compat "$pkg_dir"
}

# --- Package ----------------------------------------------------------------------

# first_line COMMAND...: the first line COMMAND prints, empty if it fails.
first_line() {
  local output
  output="$("$@" 2>/dev/null)" || output=""
  printf '%s\n' "${output%%$'\n'*}"
}

# optional_libraries: the optional libraries this build was configured with.
optional_libraries() {
  if [ "$with_sdl" -eq 1 ]; then
    printf '%s\n' sdl2
  fi
  if [ "$with_media" -eq 1 ]; then
    printf '%s\n' gstreamer ffmpeg
  fi
}

write_sources_json() {
  local out="$1" commit="unknown" dirty=false p sep="" changes
  if have git && git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    commit="$(git -C "$repo_root" rev-parse HEAD)"
    changes="$(git -C "$repo_root" status --porcelain -- runtime/build-wine.sh runtime/patches)"
    if [ -n "$changes" ]; then
      dirty=true
    fi
  fi
  local libraries=() library
  while IFS= read -r library; do
    libraries+=("$library")
  done < <(optional_libraries)
  {
    printf '{\n'
    printf '  "schema": 1,\n'
    printf '  "name": %s,\n' "$(json_str "$pkg_name")"
    printf '  "license": "LGPL-2.1-or-later",\n'
    printf '  "wine_version": %s,\n' "$(json_str "$(sed -n 1p "$src_dir/VERSION" 2>/dev/null || echo unknown)")"
    printf '  "source": {\n'
    printf '    "url": %s,\n' "$(json_str "$SOURCE_URL_BASE/crossover-sources-$crossover_version.tar.gz")"
    printf '    "sha256": %s,\n' "$(json_str "$source_sha256")"
    printf '    "size": %d,\n' "${source_size:-0}"
    printf '    "pinned": %s,\n' "$source_pinned"
    printf '    "subdir": "sources/wine"\n'
    printf '  },\n'
    printf '  "patches": ['
    for p in ${patches[@]+"${patches[@]}"}; do
      printf '%s\n    {"file": %s, "sha256": %s}' "$sep" "$(json_str "${p##*/}")" "$(json_str "$(sha256_of "$p")")"
      sep=","
    done
    if [ -n "$sep" ]; then
      printf '\n  '
    fi
    printf '],\n'
    printf '  "build_script": {\n'
    printf '    "repository": %s,\n' "$(json_str "$REPOSITORY_URL")"
    printf '    "path": "runtime/build-wine.sh",\n'
    printf '    "commit": %s,\n' "$(json_str "$commit")"
    printf '    "dirty": %s,\n' "$dirty"
    printf '    "sha256": %s\n' "$(json_str "$(sha256_of "$script_dir/build-wine.sh")")"
    printf '  },\n'
    printf '  "build": {\n'
    printf '    "host": %s,\n' "$(json_str "$HOST_TRIPLE")"
    printf '    "deployment_target": %s,\n' "$(json_str "$DEPLOYMENT_TARGET")"
    printf '    "sdk_version": %s,\n' "$(json_str "$(first_line xcrun --show-sdk-version)")"
    printf '    "clang": %s,\n' "$(json_str "$(first_line clang --version)")"
    printf '    "mingw_gcc": %s,\n' "$(json_str "$(first_line x86_64-w64-mingw32-gcc --version)")"
    # Tags from runtime/catalog.toml that the CrossOver 26 tree provides.
    printf '    "catalog_features": %s,\n' "$(json_array wow64 msync dxmt d3dmetal large-address-aware)"
    printf '    "optional_libraries": %s,\n' "$(json_array ${libraries[@]+"${libraries[@]}"})"
    printf '    "configure_args": %s\n' "$(json_array ${configure_args[@]+"${configure_args[@]}"})"
    printf '  },\n'
    printf '  "bundled_libraries": %s,\n' "$(json_array ${bundled_libs[@]+"${bundled_libs[@]}"})"
    printf '  "built_at": %s\n' "$(json_str "$(date -u +%Y-%m-%dT%H:%M:%SZ)")"
    printf '}\n'
  } >"$out"
  # plutil reads JSON; this catches a quoting bug before anyone else does.
  plutil -convert xml1 -o /dev/null "$out" || die "$out is not valid JSON (bug in this script)"
}

package_runtime() {
  local tarball="$out_dir/$pkg_name.tar.xz"
  write_sources_json "$pkg_dir/SOURCES.json"
  cp "$pkg_dir/SOURCES.json" "$out_dir/$pkg_name.SOURCES.json"
  log "packing $tarball"
  rm -f "$tarball" "$tarball.sha256"
  # No AppleDouble (._*) files, extended attributes or builder user names
  # in the archive.
  (cd "$work_dir/package" && COPYFILE_DISABLE=1 tar --no-mac-metadata \
    --uid 0 --gid 0 --uname root --gname wheel -cJf "$tarball" "$pkg_name")
  (cd "$out_dir" && printf '%s  %s\n' "$(sha256_of "$tarball")" "${tarball##*/}" >"${tarball##*/}.sha256")
  log "done: $tarball ($(size_of "$tarball") bytes)"
  cat "$tarball.sha256"
}

main() {
  parse_args "$@"
  validate_args
  prepare_dirs
  if [ "$check_only" -eq 1 ]; then
    check_prerequisites
    exit 0
  fi
  resolve_pin
  if [ "$prepare_only" -eq 0 ]; then
    check_prerequisites
  fi
  # Flags from the caller's environment would leak into both builds.
  unset CC CXX CFLAGS CXXFLAGS CPPFLAGS LDFLAGS CROSSCFLAGS CROSSLDFLAGS PKG_CONFIG_LIBDIR
  export MACOSX_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET"

  fetch_source
  extract_source
  apply_patches
  if [ "$prepare_only" -eq 1 ]; then
    log "prepared: $src_dir"
    exit 0
  fi
  build_tools
  configure_wine
  gate_configuration
  build_wine
  install_wine
  assemble_package
  strip_pe_debug_info
  relocate
  run_gates
  package_runtime
}

# Sourcing the file (scripts/test_build_wine.sh does) defines the functions
# without running a build.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
