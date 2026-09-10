#!/bin/sh
set -eu

# Build the release-shaped webserv inputs. The package source build and the
# host-side archive/signing tool are both supplied by public inputs.

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 <output-directory> [signing-key|dev]" >&2
    exit 2
fi

WEBSERV_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUTPUT_DIR_INPUT=$1
SIGNING_KEY=${2:-dev}

mkdir -p "$OUTPUT_DIR_INPUT"
OUTPUT_DIR=$(CDPATH= cd -- "$OUTPUT_DIR_INPUT" && pwd)

read_section_value() {
    file=$1
    section=$2
    key=$3
    awk -v wanted_section="$section" -v wanted_key="$key" '
        $0 == "[" wanted_section "]" { in_section = 1; next }
        $0 ~ /^\[/ { in_section = 0 }
        in_section && $1 == wanted_key && $2 == "=" {
            value = $3
            gsub(/"/, "", value)
            print value
            exit
        }
    ' "$file"
}

PACKAGE_VERSION=$(read_section_value "$WEBSERV_ROOT/package/package.toml" package version)
RELEASE_VERSION=$(read_section_value "$WEBSERV_ROOT/package/release.toml" release version)
CORE_VERSION=$(read_section_value "$WEBSERV_ROOT/Cargo.toml" package version)
SERVICE_VERSION=$(read_section_value "$WEBSERV_ROOT/service/Cargo.toml" package version)
SDK_VERSION=$(read_section_value "$WEBSERV_ROOT/package/package.toml" runtime sdk_version)

for value_name in PACKAGE_VERSION RELEASE_VERSION CORE_VERSION SERVICE_VERSION SDK_VERSION; do
    eval "value=\${$value_name}"
    if [ -z "$value" ]; then
        echo "release: could not read $value_name" >&2
        exit 2
    fi
done

if [ "$PACKAGE_VERSION" != "$RELEASE_VERSION" ] || \
   [ "$PACKAGE_VERSION" != "$CORE_VERSION" ] || \
   [ "$PACKAGE_VERSION" != "$SERVICE_VERSION" ]; then
    echo "release: package, release, core, and service versions disagree" >&2
    exit 2
fi

SDK_LOCK_VERSION=$(awk '
    $0 == "[[package]]" { in_sdk = 0; next }
    $0 == "name = \"eury-sdk\"" { in_sdk = 1; next }
    in_sdk && $1 == "version" && $2 == "=" {
        gsub(/"/, "", $3)
        print $3
        exit
    }
' "$WEBSERV_ROOT/service/Cargo.lock")
SDK_LOCK_SOURCE=$(awk '
    $0 == "[[package]]" { in_sdk = 0; next }
    $0 == "name = \"eury-sdk\"" { in_sdk = 1; next }
    in_sdk && $1 == "source" && $2 == "=" {
        gsub(/"/, "", $3)
        print $3
        exit
    }
' "$WEBSERV_ROOT/service/Cargo.lock")
if [ "$SDK_VERSION" != "$SDK_LOCK_VERSION" ]; then
    echo "release: package sdk_version $SDK_VERSION does not match Cargo.lock $SDK_LOCK_VERSION" >&2
    exit 2
fi
case "$SDK_LOCK_SOURCE" in
    registry+https://github.com/rust-lang/crates.io-index) ;;
    *)
        echo "release: eury-sdk is not resolved from crates.io" >&2
        exit 2
        ;;
esac
if grep -Fq 'git+https://github.com/EuryOS/EuryOS' "$WEBSERV_ROOT/service/Cargo.lock"; then
    echo "release: service Cargo.lock contains a private EuryOS Git dependency" >&2
    exit 2
fi

SDK_BUNDLE="$WEBSERV_ROOT/sdk/eury-sdk-bundle/0.1.0/build.sh"
BUILD_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/webserv-release-build.XXXXXX")
PACKAGE_STAGE=$(mktemp -d "${TMPDIR:-/tmp}/webserv-release-package.XXXXXX")
cleanup() {
    if [ -d "$BUILD_ROOT" ]; then
        rm -r "$BUILD_ROOT"
    fi
    if [ -d "$PACKAGE_STAGE" ]; then
        rm -r "$PACKAGE_STAGE"
    fi
}
trap cleanup EXIT HUP INT TERM

CARGO_TARGET_DIR="$BUILD_ROOT/cargo-target" sh "$SDK_BUNDLE" "$WEBSERV_ROOT/service"
ELF="$BUILD_ROOT/cargo-target/aarch64-euryos-driver/release/webserv"
if [ ! -s "$ELF" ]; then
    echo "release: service build did not produce $ELF" >&2
    exit 1
fi

ELF_NAME="webserv-$PACKAGE_VERSION-aarch64-euryos-driver.elf"
PACKAGE_NAME="webserv-$PACKAGE_VERSION-aarch64-euryos-driver.eury"
RELEASE_NAME="webserv-$PACKAGE_VERSION-release.toml"
CHECKSUM_NAME="webserv-$PACKAGE_VERSION-SHA256SUMS"
cp "$ELF" "$OUTPUT_DIR/$ELF_NAME"
mkdir -p "$PACKAGE_STAGE/bin"
cp "$ELF" "$PACKAGE_STAGE/bin/main.elf"

PACKAGE_TOOL=${EURY_PACKAGE_BIN:-eury-package}
if [ -x "$PACKAGE_TOOL" ]; then
    PACKAGE_TOOL_PATH=$PACKAGE_TOOL
else
    PACKAGE_TOOL_PATH=$(command -v "$PACKAGE_TOOL" 2>/dev/null || true)
fi
if [ -z "$PACKAGE_TOOL_PATH" ] || [ ! -x "$PACKAGE_TOOL_PATH" ]; then
    echo "release: eury-package is required; install eury-package 0.1.0 or set EURY_PACKAGE_BIN" >&2
    exit 2
fi

"$PACKAGE_TOOL_PATH" package "$PACKAGE_STAGE" \
    --metadata "$WEBSERV_ROOT/package/package.toml" \
    --sign "$SIGNING_KEY" \
    -o "$OUTPUT_DIR/$PACKAGE_NAME"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

SOURCE_REVISION=$(git -C "$WEBSERV_ROOT" rev-parse HEAD)
MANIFEST_SHA256=$(sha256_file "$WEBSERV_ROOT/package/package.toml")
ARTIFACT_SHA256=$(sha256_file "$OUTPUT_DIR/$PACKAGE_NAME")
SIGNATURE_REFERENCE="embedded:$SIGNING_KEY"
{
    printf '%s\n' '[release]'
    printf 'package = "webserv"\n'
    printf 'version = "%s"\n' "$PACKAGE_VERSION"
    printf 'sdk = "eury-sdk"\n'
    printf 'sdk_version = "%s"\n' "$SDK_VERSION"
    printf 'source_revision = "%s"\n' "$SOURCE_REVISION"
    printf 'manifest_sha256 = "%s"\n\n' "$MANIFEST_SHA256"
    printf '%s\n' '[[artifact]]'
    printf 'target = "aarch64-euryos-driver"\n'
    printf 'os_abi = "euryos-abi-v1"\n'
    printf 'profile = "server"\n'
    printf 'path = "%s"\n' "$PACKAGE_NAME"
    printf 'sha256 = "%s"\n' "$ARTIFACT_SHA256"
    printf 'signature = "%s"\n' "$SIGNATURE_REFERENCE"
} > "$OUTPUT_DIR/$RELEASE_NAME"

(
    cd "$OUTPUT_DIR"
    printf '%s  %s\n' "$(sha256_file "$PACKAGE_NAME")" "$PACKAGE_NAME"
    printf '%s  %s\n' "$(sha256_file "$ELF_NAME")" "$ELF_NAME"
) > "$OUTPUT_DIR/$CHECKSUM_NAME"

printf 'release: webserv %s\n' "$PACKAGE_VERSION"
printf '  package: %s\n' "$OUTPUT_DIR/$PACKAGE_NAME"
printf '  elf: %s\n' "$OUTPUT_DIR/$ELF_NAME"
printf '  metadata: %s\n' "$OUTPUT_DIR/$RELEASE_NAME"
