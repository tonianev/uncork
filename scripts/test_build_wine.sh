#!/usr/bin/env bash
# test_build_wine.sh: offline tests for runtime/build-wine.sh.
#
# Sources the build script (which then defines its functions without
# building anything) and checks argument handling, the source pin, the JSON
# writer, and the relocation and gates against a few tiny x86_64 Mach-O
# files compiled here. No network, no Wine, no mingw-w64; needs macOS with
# the Xcode Command Line Tools. Runs in CI and in `just ci`.
#
# Usage: scripts/test_build_wine.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd -P)"
# shellcheck source=runtime/build-wine.sh
. "$root/runtime/build-wine.sh"

if [ "$(uname -s)" != Darwin ] || ! command -v clang >/dev/null 2>&1; then
  echo "skip: test_build_wine.sh needs macOS with the Xcode Command Line Tools"
  exit 0
fi

t="$(mktemp -d "${TMPDIR:-/tmp}/uncork-test-build-wine.XXXXXX")"
t="$(cd "$t" && pwd -P)"
trap 'rm -rf "$t"' EXIT
tmp_dir="$t/tmp"
mkdir -p "$tmp_dir"

passed=0
pass() {
  passed=$((passed + 1))
  printf 'ok %d - %s\n' "$passed" "$1"
}
fail() {
  printf 'not ok - %s\n' "$1" >&2
  exit 1
}
# expect_eq NAME ACTUAL EXPECTED
expect_eq() {
  [ "$2" = "$3" ] || fail "$1: expected '$3', got '$2'"
  pass "$1"
}
# expect_ok NAME COMMAND...: COMMAND succeeds (its output goes to a log).
expect_ok() {
  local name="$1"
  shift
  if ! ("$@") >"$t/last.log" 2>&1; then
    cat "$t/last.log" >&2
    fail "$name: expected success"
  fi
  pass "$name"
}
# expect_fail NAME COMMAND...: COMMAND fails. It runs in a subshell, so a
# `die` inside it ends only the subshell.
expect_fail() {
  local name="$1"
  shift
  if ("$@") >"$t/last.log" 2>&1; then
    fail "$name: expected failure"
  fi
  pass "$name"
}
# expect_contains NAME HAYSTACK NEEDLE: NEEDLE is one of HAYSTACK's lines.
expect_contains() {
  grep -qxF -- "$3" <<<"$2" || fail "$1: '$3' not in: $(tr '\n' ' ' <<<"$2")"
  pass "$1"
}

x86_64_cc() {
  clang -arch x86_64 -mmacosx-version-min=15.0 "$@"
}

# --- arguments -------------------------------------------------------------------

test_arguments() {
  expect_ok "is_version accepts X.Y.Z" is_version 26.3.0
  expect_fail "is_version rejects X.Y" is_version 26.3
  expect_fail "is_version rejects four parts" is_version 26.3.0.1
  expect_fail "is_version rejects shell text" is_version '26.3.0;true'
  expect_fail "is_version rejects empty" is_version ''
  expect_ok "is_sha256 accepts 64 lowercase hex" is_sha256 "$(printf 'a%.0s' $(seq 64))"
  expect_fail "is_sha256 rejects uppercase" is_sha256 "$(printf 'A%.0s' $(seq 64))"
  expect_fail "is_sha256 rejects TODO" is_sha256 TODO
  expect_ok "has_space finds a space" has_space "/a b"
  expect_fail "has_space passes a plain path" has_space /a/b
  expect_fail "require_value rejects a missing value" require_value --jobs
  expect_fail "require_value rejects an option as value" require_value --jobs --check-only

  parse_args --crossover-version=26.2.0 --deps-prefix /x --deps-prefix=/y --without-media --jobs 3
  expect_eq "parse_args: --opt=value form" "$crossover_version" 26.2.0
  expect_eq "parse_args: repeated --deps-prefix" "${deps_prefixes[*]}" "/x /y"
  expect_eq "parse_args: --without-media" "$with_media" 0
  expect_eq "parse_args: --jobs" "$jobs" 3
  expect_fail "parse_args rejects unknown options" parse_args --frobnicate
  crossover_version="$DEFAULT_CROSSOVER_VERSION"
  deps_prefixes=()
  with_media=1
  jobs=2
}

# --- source pin ------------------------------------------------------------------

test_pins() {
  local entry
  for entry in "${CROSSOVER_PINS[@]}"; do
    local version size sha
    IFS='|' read -r version size sha <<<"$entry"
    is_version "$version" || fail "pin table: bad version '$version'"
    case "$size" in '' | *[!0-9]*) fail "pin table: bad size '$size' for $version" ;; esac
    if [ "$sha" != TODO ]; then
      is_sha256 "$sha" || fail "pin table: bad sha256 '$sha' for $version"
    fi
  done
  pass "pin table is well formed"
  expect_eq "pin_for finds the default version" "$(pin_for "$DEFAULT_CROSSOVER_VERSION" | cut -d'|' -f1)" 149054023
  expect_fail "pin_for fails for an unknown version" pin_for 1.2.3

  crossover_version=1.2.3
  allow_unpinned=0
  expect_fail "resolve_pin refuses an unknown version" resolve_pin
  allow_unpinned=1
  resolve_pin 2>/dev/null
  expect_eq "resolve_pin allows it with --allow-unpinned" "$source_pinned" false

  # A pinned entry, against a real file.
  printf 'not really a tarball\n' >"$t/source.tar.gz"
  local hash
  hash="$(sha256_of "$t/source.tar.gz")"
  expect_eq "sha256_of matches shasum" "$hash" "$(shasum -a 256 "$t/source.tar.gz" | cut -d' ' -f1)"
  CROSSOVER_PINS=("1.2.3|21|$hash")
  allow_unpinned=0
  resolve_pin
  expect_eq "resolve_pin accepts a pinned version" "$source_pinned" true
  expect_ok "verify_tarball accepts the pinned file" verify_tarball "$t/source.tar.gz"
  printf 'tampered!!!!!!!!!!!!\n' >"$t/tampered.tar.gz"
  expect_fail "verify_tarball rejects a different hash" verify_tarball "$t/tampered.tar.gz"
  printf 'short\n' >"$t/short.tar.gz"
  expect_fail "verify_tarball rejects a different size" verify_tarball "$t/short.tar.gz"
  CROSSOVER_PINS=("1.2.3|21|NOT-A-HASH")
  expect_fail "resolve_pin rejects a malformed pin" resolve_pin
}

# --- helpers ---------------------------------------------------------------------

test_helpers() {
  expect_eq "relpath to the same dir" "$(relpath /p/lib /p/lib)" .
  expect_eq "relpath two levels up" "$(relpath /p/lib/wine/x86_64-unix /p/lib)" ../..
  expect_eq "relpath to a sibling" "$(relpath /p/bin /p/lib)" ../lib
  expect_eq "relpath down" "$(relpath /p /p/lib/wine)" lib/wine
  expect_eq "relpath across" "$(relpath /p/lib/gstreamer-1.0 /p/lib)" ..
  expect_eq "version_code 15.0" "$(version_code 15.0)" 150000
  expect_eq "version_code 10.13" "$(version_code 10.13)" 101300
  expect_eq "version_code 26.0.1" "$(version_code 26.0.1)" 260001
  expect_eq "version_code 9" "$(version_code 9)" 90000
  expect_eq "major_version of bison" "$(major_version 'bison (GNU Bison) 3.8.2')" 3
  expect_eq "major_version of Apple's bison" "$(major_version 'bison (GNU Bison) 2.3')" 2
  expect_eq "first_line" "$(first_line printf 'a\nb\n')" a
  expect_eq "first_line of a failing command" "$(first_line false)" ""
  expect_eq "json_array" "$(json_array a 'b c')" '["a", "b c"]'
  expect_eq "json_array of nothing" "$(json_array)" '[]'

  local weird='quote " backslash \ tab	newline
end'
  printf '{"k": %s}\n' "$(json_str "$weird")" >"$t/weird.json"
  expect_eq "json_str round-trips through plutil" "$(plutil -extract k raw -o - "$t/weird.json")" "$weird"
}

# --- patches ---------------------------------------------------------------------

test_patches() {
  local real_script_dir="$script_dir"
  script_dir="$t/patch-test"
  src_dir="$t/patch-test/wine"
  mkdir -p "$script_dir/patches" "$src_dir"
  apply_patches 2>/dev/null
  expect_eq "apply_patches with an empty queue" "${#patches[@]}" 0

  printf 'one\ntwo\nthree\n' >"$src_dir/f.txt"
  # 0002 only applies on top of 0001, so this also checks the order.
  printf -- '--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n' \
    >"$script_dir/patches/0001-upper.patch"
  printf -- '--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n one\n-TWO\n+2\n three\n' \
    >"$script_dir/patches/0002-digit.patch"
  printf 'not a patch\n' >"$script_dir/patches/notes.txt"
  apply_patches 2>/dev/null
  expect_eq "apply_patches applies the queue in name order" "$(tr '\n' ' ' <"$src_dir/f.txt")" "one 2 three "
  expect_eq "apply_patches takes only *.patch" "${#patches[@]}" 2
  expect_fail "apply_patches stops on a patch that no longer applies" apply_patches
  [ ! -e "$src_dir/f.txt.rej" ] || fail "a failed dry run left a .rej file"
  pass "a failed dry run leaves the tree alone"
  script_dir="$real_script_dir"
  patches=()
}

# --- relocation ------------------------------------------------------------------

# A dependency tree like the real one: libfoo (absolute install name) under
# libbar, libbaz with an @rpath name, libdlonly that only a SONAME names, and
# a GStreamer plugin; a Wine-like Unix module and server that link them.
build_fixture() {
  local d1="$t/deps1" d2="$t/deps2"
  mkdir -p "$d1/lib/gstreamer-1.0" "$d2/lib" "$t/src"
  printf 'int foo(void) { return 1; }\n' >"$t/src/foo.c"
  printf 'int foo(void); int bar(void) { return foo(); }\n' >"$t/src/bar.c"
  printf 'int baz(void) { return 3; }\n' >"$t/src/baz.c"
  printf 'int dlonly(void) { return 4; }\n' >"$t/src/dlonly.c"
  printf 'int foo(void); int plugin(void) { return foo(); }\n' >"$t/src/plugin.c"
  printf 'int bar(void); int baz(void); int unix_call(void) { return bar() + baz(); }\n' >"$t/src/module.c"
  printf 'int bar(void); int main(void) { return bar(); }\n' >"$t/src/server.c"

  x86_64_cc -dynamiclib "$t/src/foo.c" -install_name "$d1/lib/libfoo.1.dylib" -o "$d1/lib/libfoo.1.dylib"
  ln -s libfoo.1.dylib "$d1/lib/libfoo.dylib"
  x86_64_cc -dynamiclib "$t/src/dlonly.c" -install_name "$d1/lib/libdlonly.2.dylib" -o "$d1/lib/libdlonly.2.dylib"
  x86_64_cc -dynamiclib "$t/src/plugin.c" -L"$d1/lib" -lfoo \
    -install_name "$d1/lib/gstreamer-1.0/libgstfake.dylib" -o "$d1/lib/gstreamer-1.0/libgstfake.dylib"
  x86_64_cc -dynamiclib "$t/src/bar.c" -L"$d1/lib" -lfoo -install_name "$d2/lib/libbar.dylib" -o "$d2/lib/libbar.dylib"
  x86_64_cc -dynamiclib "$t/src/baz.c" -install_name "@rpath/libbaz.dylib" -o "$d2/lib/libbaz.dylib"

  pkg_dir="$t/pkg"
  libdir="$pkg_dir/lib"
  mkdir -p "$pkg_dir/bin" "$pkg_dir/lib/wine/x86_64-unix" "$pkg_dir/lib/wine/x86_64-windows" "$pkg_dir/share"
  x86_64_cc -bundle "$t/src/module.c" -L"$d2/lib" -lbar -lbaz \
    -Wl,-headerpad_max_install_names -o "$pkg_dir/lib/wine/x86_64-unix/ntdll.so"
  x86_64_cc "$t/src/server.c" -L"$d2/lib" -lbar -Wl,-headerpad_max_install_names -o "$pkg_dir/bin/wineserver"
  printf 'MZ not a Mach-O\n' >"$pkg_dir/lib/wine/x86_64-windows/ntdll.dll"
  printf 'text\n' >"$pkg_dir/share/readme"

  build_dir="$t/build"
  mkdir -p "$build_dir/include"
  cat >"$build_dir/include/config.h" <<'EOF'
#define SONAME_LIBDLONLY "libdlonly.2.dylib"
#define SONAME_LIBSYSTEMONLY "libsystemonly.1.dylib"
EOF
  deps_prefixes=("$d1" "$d2")
  with_media=1
}

test_relocation() {
  build_fixture
  local files
  files="$(macho_files "$pkg_dir")"
  files="${files//"$pkg_dir"\//}"
  expect_eq "macho_files skips PE and text files" "$(tr '\n' ' ' <<<"$files")" \
    "bin/wineserver lib/wine/x86_64-unix/ntdll.so "
  expect_contains "load_commands lists an absolute dependency" \
    "$(load_commands "$pkg_dir/bin/wineserver")" "$t/deps2/lib/libbar.dylib"
  expect_eq "install_id of a dylib" "$(install_id "$t/deps2/lib/libbaz.dylib")" "@rpath/libbaz.dylib"
  expect_eq "install_id of an executable is empty" "$(install_id "$pkg_dir/bin/wineserver")" ""
  expect_eq "minos_of" "$(minos_of "$pkg_dir/bin/wineserver")" 15.0
  expect_fail "gate_relocatable fails before relocation" gate_relocatable "$pkg_dir"

  relocate 2>"$t/relocate.log" || {
    cat "$t/relocate.log" >&2
    fail "relocate failed"
  }
  pass "relocate runs"

  local lib
  for lib in libfoo.1.dylib libbar.dylib libbaz.dylib libdlonly.2.dylib gstreamer-1.0/libgstfake.dylib; do
    [ -f "$libdir/$lib" ] || fail "relocate did not bundle $lib"
  done
  pass "relocate bundles linked, @rpath, dlopened and plugin libraries"
  [ ! -e "$libdir/libsystemonly.1.dylib" ] || fail "a SONAME no prefix has was bundled"
  pass "relocate leaves names no prefix provides to the system"

  local module="$pkg_dir/lib/wine/x86_64-unix/ntdll.so"
  expect_contains "module: absolute dependency rewritten" "$(load_commands "$module")" "@loader_path/../../libbar.dylib"
  expect_contains "module: @rpath dependency rewritten" "$(load_commands "$module")" "@loader_path/../../libbaz.dylib"
  expect_contains "server: dependency rewritten" "$(load_commands "$pkg_dir/bin/wineserver")" "@loader_path/../lib/libbar.dylib"
  expect_contains "bundled dylib: dependency rewritten" "$(load_commands "$libdir/libbar.dylib")" "@loader_path/libfoo.1.dylib"
  expect_eq "bundled dylib: install name rewritten" "$(install_id "$libdir/libbar.dylib")" "@loader_path/libbar.dylib"
  expect_contains "plugin: dependency rewritten" \
    "$(load_commands "$libdir/gstreamer-1.0/libgstfake.dylib")" "@loader_path/../libfoo.1.dylib"
  expect_eq "module: rpath to lib/" "$(rpaths_of "$module")" "@loader_path/../../"
  add_rpath "$module" "@loader_path/../../"
  expect_eq "add_rpath does not duplicate" "$(rpaths_of "$module")" "@loader_path/../../"
  expect_eq "x86_64 slice kept" "$(lipo -archs "$libdir/libbar.dylib")" x86_64

  local file
  while IFS= read -r file; do
    codesign --verify --strict "$file" 2>/dev/null || fail "signature does not verify: $file"
  done < <(macho_files "$pkg_dir")
  pass "every Mach-O is validly signed after relocation"
  expect_ok "gate_relocatable passes after relocation" gate_relocatable "$pkg_dir"
  expect_ok "gate_deployment_target passes at the floor" gate_deployment_target "$pkg_dir"

  clang -arch x86_64 -mmacosx-version-min=26.0 -dynamiclib "$t/src/foo.c" -o "$libdir/libtoonew.dylib"
  expect_fail "gate_deployment_target rejects a newer minos" gate_deployment_target "$pkg_dir"
  rm "$libdir/libtoonew.dylib"

  x86_64_cc -dynamiclib "$t/src/foo.c" -install_name /opt/elsewhere/libabs.dylib -o "$libdir/libabs.dylib"
  expect_fail "gate_relocatable rejects an absolute install name" gate_relocatable "$pkg_dir"
  rm "$libdir/libabs.dylib"
}

# --- tree and link gates ---------------------------------------------------------

test_tree_gates() {
  local dir="$t/tree" i
  mkdir -p "$dir/bin" "$dir/lib/wine/x86_64-unix" "$dir/lib/wine/x86_64-windows" "$dir/lib/wine/i386-windows"
  printf '#!/bin/sh\n' >"$dir/lib/wine/x86_64-unix/wine"
  chmod +x "$dir/lib/wine/x86_64-unix/wine"
  ln -s ../lib/wine/x86_64-unix/wine "$dir/bin/wine"
  : >"$dir/bin/wineserver"
  : >"$dir/lib/wine/x86_64-unix/ntdll.so"
  : >"$dir/lib/wine/x86_64-windows/ntdll.dll"
  : >"$dir/lib/wine/i386-windows/ntdll.dll"
  expect_fail "gate_tree rejects a thin i386 half" gate_tree "$dir"
  for i in $(seq 1 "$MIN_I386_PE_FILES"); do
    : >"$dir/lib/wine/i386-windows/module$i.dll"
  done
  expect_ok "gate_tree accepts a complete tree" gate_tree "$dir"
  rm "$dir/lib/wine/x86_64-unix/ntdll.so"
  expect_fail "gate_tree rejects a missing ntdll.so" gate_tree "$dir"
  : >"$dir/lib/wine/x86_64-unix/ntdll.so"

  expect_ok "gate_links accepts relative symlinks inside the tree" gate_links "$dir"
  ln "$dir/bin/wineserver" "$dir/bin/hardlink"
  gate_links "$dir" 2>/dev/null
  expect_eq "gate_links turns hard links into copies" "$(find "$dir" -type f -links +1 | wc -l | tr -d ' ')" 0
  ln -s /etc/hosts "$dir/bin/absolute"
  expect_fail "gate_links rejects an absolute symlink" gate_links "$dir"
  rm "$dir/bin/absolute"
  ln -s ../../outside "$dir/bin/escape"
  expect_fail "gate_links rejects a symlink out of the tree" gate_links "$dir"
  rm "$dir/bin/escape"
  ln -s missing "$dir/bin/dangling"
  expect_fail "gate_links rejects a dangling symlink" gate_links "$dir"
  rm "$dir/bin/dangling"
}

# --- SOURCES.json ----------------------------------------------------------------

test_sources_json() {
  src_dir="$t/src-wine"
  mkdir -p "$src_dir"
  printf 'Wine version 11.0\n' >"$src_dir/VERSION"
  mkdir -p "$t/patches"
  printf -- '--- a/x\n+++ b/x\n' >"$t/patches/0001-example.patch"
  patches=("$t/patches/0001-example.patch")
  pkg_name="uncork-wine-26.3.0-x86_64"
  crossover_version=26.3.0
  source_sha256="$(printf 'b%.0s' $(seq 64))"
  source_size=149054023
  source_pinned=true
  with_sdl=0
  with_media=1
  configure_args=(--host=x86_64-apple-darwin24 "--enable-archs=i386,x86_64")
  bundled_libs=(libfoo.1.dylib)
  write_sources_json "$t/SOURCES.json"
  pass "write_sources_json writes valid JSON"
  local json="$t/SOURCES.json"
  expect_eq "SOURCES.json: source sha256" "$(plutil -extract source.sha256 raw -o - "$json")" "$source_sha256"
  expect_eq "SOURCES.json: source size" "$(plutil -extract source.size raw -o - "$json")" 149054023
  expect_eq "SOURCES.json: pinned" "$(plutil -extract source.pinned raw -o - "$json")" true
  expect_eq "SOURCES.json: patch file" "$(plutil -extract patches.0.file raw -o - "$json")" 0001-example.patch
  expect_eq "SOURCES.json: patch hash" "$(plutil -extract patches.0.sha256 raw -o - "$json")" \
    "$(sha256_of "$t/patches/0001-example.patch")"
  expect_eq "SOURCES.json: script hash" "$(plutil -extract build_script.sha256 raw -o - "$json")" \
    "$(sha256_of "$root/runtime/build-wine.sh")"
  expect_eq "SOURCES.json: optional libraries" "$(plutil -extract build.optional_libraries.0 raw -o - "$json")" gstreamer
  expect_eq "SOURCES.json: configure args" "$(plutil -extract build.configure_args.1 raw -o - "$json")" "--enable-archs=i386,x86_64"

  patches=()
  bundled_libs=()
  write_sources_json "$t/SOURCES-empty.json"
  expect_eq "SOURCES.json: no patches" "$(plutil -extract patches json -o - "$t/SOURCES-empty.json")" "[]"
  expect_eq "SOURCES.json: no bundled libraries" "$(plutil -extract bundled_libraries json -o - "$t/SOURCES-empty.json")" "[]"
}

# --- packaging -------------------------------------------------------------------

# Runs after test_sources_json, whose globals describe the build.
test_package() {
  work_dir="$t/work"
  out_dir="$t/out"
  pkg_dir="$work_dir/package/$pkg_name"
  mkdir -p "$pkg_dir/bin" "$out_dir"
  printf '#!/bin/sh\n' >"$pkg_dir/bin/wineserver"
  ln -s wineserver "$pkg_dir/bin/wine"
  package_runtime >/dev/null 2>&1
  local tarball="$out_dir/$pkg_name.tar.xz" listing
  [ -f "$tarball" ] || fail "package_runtime wrote no tarball"
  listing="$(tar -tJf "$tarball")"
  expect_contains "tarball holds SOURCES.json" "$listing" "$pkg_name/SOURCES.json"
  expect_contains "tarball keeps symlinks" "$listing" "$pkg_name/bin/wine"
  grep -q '/\._' <<<"$listing" && fail "tarball holds AppleDouble files"
  pass "tarball holds no AppleDouble files"
  expect_ok "checksum file verifies" sh -c "cd '$out_dir' && shasum -a 256 -c '$pkg_name.tar.xz.sha256'"
  expect_ok "SOURCES.json is published beside the tarball" test -f "$out_dir/$pkg_name.SOURCES.json"
}

test_arguments
test_pins
test_helpers
test_patches
test_relocation
test_tree_gates
test_sources_json
test_package
printf 'test_build_wine: %d checks passed\n' "$passed"
