#!/bin/sh
# Offline verification of the std-only crates on Unix. This does NOT validate the GUI or Cargo resolution.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
build=target/standalone-check
mkdir -p "$build"

rustc --edition=2024 -D warnings -D unsafe_code --crate-name lisnap_core --crate-type rlib crates/lisnap-core/src/lib.rs --out-dir "$build"
rustc --edition=2024 -D warnings -D unsafe_code --crate-name lisnap_platform --crate-type rlib crates/lisnap-platform/src/lib.rs --extern lisnap_core="$build/liblisnap_core.rlib" -L dependency="$build" --out-dir "$build"

for name in geometry image; do
    rustc --edition=2024 -D warnings -D unsafe_code --test "crates/lisnap-core/tests/$name.rs" --extern lisnap_core="$build/liblisnap_core.rlib" -L dependency="$build" -o "$build/core-$name"
    "$build/core-$name"
done
rustc --edition=2024 -D warnings -D unsafe_code --test crates/lisnap-platform/src/lib.rs --extern lisnap_core="$build/liblisnap_core.rlib" -L dependency="$build" -o "$build/platform-tests"
"$build/platform-tests"
rustc --edition=2024 -D warnings -D unsafe_code --test apps/lisnap/src/cli.rs --extern lisnap_platform="$build/liblisnap_platform.rlib" -L dependency="$build" -o "$build/cli-tests"
"$build/cli-tests"

version=$(awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml)
test -n "$version"
CARGO_PKG_VERSION="$version" rustc --edition=2024 -D warnings -D unsafe_code --crate-name lisnap apps/lisnap/src/main.rs --extern lisnap_platform="$build/liblisnap_platform.rlib" -L dependency="$build" -o "$build/lisnap"
"$build/lisnap" --version
"$build/lisnap" --list-backends
printf '\nOffline core/CLI verification passed. GUI compilation was not tested.\n'
