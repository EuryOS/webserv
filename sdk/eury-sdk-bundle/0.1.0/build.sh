#!/bin/sh
set -eu

# This wrapper turns the bundle's target template into a target specification
# containing an absolute linker path. rust-lld resolves -T paths from Cargo's
# build working directory, not from the target template's directory.

SDK_BUNDLE_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SDK_TOOLCHAIN=nightly-2026-07-26
SDK_TARGET_NAME=aarch64-euryos-driver

if [ "$#" -gt 0 ]; then
    SDK_PROJECT_INPUT=$1
    shift
else
    SDK_PROJECT_INPUT=.
fi

SDK_PROJECT_ROOT=$(CDPATH= cd -- "$SDK_PROJECT_INPUT" && pwd)
SDK_PROJECT_MANIFEST=$SDK_PROJECT_ROOT/Cargo.toml
SDK_TEMPLATE=$SDK_BUNDLE_ROOT/targets/$SDK_TARGET_NAME.json.in
SDK_LINKER=$SDK_BUNDLE_ROOT/linker/driver.ld

if [ ! -f "$SDK_PROJECT_MANIFEST" ]; then
    echo "EuryOS SDK: no Cargo.toml at $SDK_PROJECT_ROOT" >&2
    exit 2
fi
if [ ! -f "$SDK_TEMPLATE" ] || [ ! -f "$SDK_LINKER" ]; then
    echo "EuryOS SDK: incomplete 0.1.0 bundle at $SDK_BUNDLE_ROOT" >&2
    exit 2
fi

SDK_TARGET_SPEC_DIR=$(mktemp -d "${TMPDIR:-/tmp}/eury-sdk-target.XXXXXX")
SDK_CLEANUP() {
    if [ -d "$SDK_TARGET_SPEC_DIR" ]; then
        rm -r "$SDK_TARGET_SPEC_DIR"
    fi
}
trap SDK_CLEANUP EXIT HUP INT TERM

SDK_LINKER_SED=$(printf '%s' "$SDK_LINKER" | sed 's/[\\&|]/\\&/g')
sed "s|__EURY_SDK_LINKER__|$SDK_LINKER_SED|g" "$SDK_TEMPLATE" \
    > "$SDK_TARGET_SPEC_DIR/$SDK_TARGET_NAME.json"

cd "$SDK_PROJECT_ROOT"
rustup run "$SDK_TOOLCHAIN" cargo build \
    --locked \
    --release \
    --manifest-path "$SDK_PROJECT_MANIFEST" \
    --target "$SDK_TARGET_SPEC_DIR/$SDK_TARGET_NAME.json" \
    -Zbuild-std=core,alloc,compiler_builtins \
    -Zbuild-std-features=compiler-builtins-mem \
    -Zjson-target-spec \
    "$@"
