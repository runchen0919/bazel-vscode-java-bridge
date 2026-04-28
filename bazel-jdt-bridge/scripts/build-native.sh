#!/usr/bin/env bash
# Build native libraries for bazel-jdt-bridge
# Supports cross-compilation via cargo-zigbuild
#
# Usage:
#   ./build-native.sh              # Build all targets
#   ./build-native.sh --target x86_64-unknown-linux-gnu  # Build specific target
#   ./build-native.sh --help       # Show help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# All supported targets
ALL_TARGETS=(
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
    "x86_64-apple-darwin"
    "aarch64-apple-darwin"
    "x86_64-pc-windows-gnu"
)

# Platform directory mapping
get_platform_dir() {
    local target="$1"
    case "$target" in
        *linux*)  echo "linux-${target%%-*}" ;;
        *darwin*) echo "darwin-${target%%-*}" ;;
        *windows*) echo "windows-${target%%-*}" ;;
        *)        echo "unknown" ;;
    esac
}

# Library name mapping
get_lib_name() {
    local target="$1"
    case "$target" in
        *linux*)   echo "libbazel_jdt_core.so" ;;
        *darwin*)  echo "libbazel_jdt_core.dylib" ;;
        *windows*) echo "bazel_jdt_core.dll" ;;
        *)         echo "libbazel_jdt_core.so" ;;
    esac
}

# Detect native target for current host
get_native_target() {
    local os="$(uname -s)"
    local arch="$(uname -m)"
    
    case "$os" in
        Linux)
            echo "${arch}-unknown-linux-gnu"
            ;;
        Darwin)
            echo "${arch}-apple-darwin"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "${arch}-pc-windows-gnu"
            ;;
        *)
            echo "unknown"
            ;;
    esac
}

# Check if target is native (matches current host)
is_native_target() {
    local target="$1"
    local native="$(get_native_target)"
    [[ "$target" == "$native" ]]
}

# Build for a single target
build_target() {
    local target="$1"
    local platform_dir
    local lib_name
    local native_dir
    local source_path
    
    echo "--- Building for $target ---"
    
    platform_dir="$(get_platform_dir "$target")"
    lib_name="$(get_lib_name "$target")"
    native_dir="$PROJECT_ROOT/java-bridge/src/main/resources/native/$platform_dir"
    
    # Choose build tool: cargo-zigbuild for cross, regular cargo for native
    if is_native_target "$target" && ! command -v cargo-zigbuild &>/dev/null; then
        echo "  Using cargo (native target)"
        cargo build --release -p bazel-jdt-core
        source_path="$PROJECT_ROOT/target/release/$lib_name"
    elif command -v cargo-zigbuild &>/dev/null; then
        echo "  Using cargo-zigbuild"
        cargo zigbuild --target "$target" --release -p bazel-jdt-core
        source_path="$PROJECT_ROOT/target/$target/release/$lib_name"
    else
        echo "  ERROR: cargo-zigbuild not found (required for cross-compilation)"
        echo "  Install with: cargo install cargo-zigbuild"
        return 1
    fi
    
    # Copy output to resources directory
    mkdir -p "$native_dir"
    if [[ -f "$source_path" ]]; then
        cp "$source_path" "$native_dir/"
        echo "  -> $native_dir/$lib_name"
    else
        echo "  WARNING: Library not found at $source_path"
        return 1
    fi
}

# Show help
show_help() {
    cat <<EOF
Build native libraries for bazel-jdt-bridge

Usage:
  $0 [OPTIONS]

Options:
  --target TARGET    Build for specific target (can be used multiple times)
  --all              Build all targets (default)
  --list             List available targets
  --help             Show this help

Supported targets:
$(printf '  - %s\n' "${ALL_TARGETS[@]}")

Output:
  Libraries are placed in:
    java-bridge/src/main/resources/native/<platform>/

  Platform mapping:
    x86_64-unknown-linux-gnu  -> linux-x86_64/
    aarch64-unknown-linux-gnu -> linux-aarch64/
    x86_64-apple-darwin       -> darwin-x86_64/
    aarch64-apple-darwin      -> darwin-aarch64/
    x86_64-pc-windows-gnu     -> windows-x86_64/

Requirements:
  - cargo-zigbuild for cross-compilation
  - Regular cargo works for native target only
EOF
}

# Parse arguments
TARGETS_TO_BUILD=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            shift
            if [[ $# -eq 0 ]]; then
                echo "ERROR: --target requires an argument"
                exit 1
            fi
            TARGETS_TO_BUILD+=("$1")
            shift
            ;;
        --all)
            TARGETS_TO_BUILD=("${ALL_TARGETS[@]}")
            shift
            ;;
        --list)
            printf '%s\n' "${ALL_TARGETS[@]}"
            exit 0
            ;;
        --help|-h)
            show_help
            exit 0
            ;;
        *)
            echo "ERROR: Unknown option: $1"
            echo "Run '$0 --help' for usage"
            exit 1
            ;;
    esac
done

# Default to all targets if none specified
if [[ ${#TARGETS_TO_BUILD[@]} -eq 0 ]]; then
    TARGETS_TO_BUILD=("${ALL_TARGETS[@]}")
fi

# Validate targets
for target in "${TARGETS_TO_BUILD[@]}"; do
    valid=false
    for t in "${ALL_TARGETS[@]}"; do
        if [[ "$target" == "$t" ]]; then
            valid=true
            break
        fi
    done
    if [[ "$valid" == false ]]; then
        echo "ERROR: Invalid target: $target"
        echo "Valid targets: ${ALL_TARGETS[*]}"
        exit 1
    fi
done

cd "$PROJECT_ROOT"

echo "Building native libraries..."
echo "Targets: ${TARGETS_TO_BUILD[*]}"
echo ""

FAILED=0
for target in "${TARGETS_TO_BUILD[@]}"; do
    if ! build_target "$target"; then
        FAILED=$((FAILED + 1))
    fi
done

echo ""
if [[ $FAILED -eq 0 ]]; then
    echo "Done. Built ${#TARGETS_TO_BUILD[@]} target(s)."
else
    echo "Done with errors. $FAILED target(s) failed."
    exit 1
fi
