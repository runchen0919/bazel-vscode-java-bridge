#!/usr/bin/env bash
# Build and deploy for local debugging.
#
# Builds Rust native lib → Java OSGi bundle → copies JAR to vscode-extension/server/
# Optionally also builds the TS extension bundle.
#
# Usage:
#   ./scripts/build-for-debug.sh           # Full build (Rust + Java + TS)
#   ./scripts/build-for-debug.sh --skip-ts # Only Rust + Java (JAR to server/)
#   ./scripts/build-for-debug.sh --skip-rust # Skip Rust rebuild (Java + TS only)
#   ./scripts/build-for-debug.sh --clean   # Clear caches + stale build artifacts
#   ./scripts/build-for-debug.sh --help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build"
SERVER_JAR="$PROJECT_ROOT/vscode-extension/server/com.bazel.jdt.jar"

SKIP_TS=false
SKIP_RUST=false
CLEAN=false

# Classes to verify after build (catches ClassFormatError early)
VERIFY_CLASSES=(
    "com.bazel.jdt.BazelBridge"
    "com.bazel.jdt.BazelProjectImporter"
    "com.bazel.jdt.BazelBuildSupport"
    "com.bazel.jdt.BazelClasspathManager"
    "com.bazel.jdt.BazelCommandHandler"
)

# --- Parse args ---
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-ts)     SKIP_TS=true;   shift ;;
        --skip-rust)   SKIP_RUST=true; shift ;;
        --clean)       CLEAN=true;     shift ;;
        --help|-h)
            cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --skip-ts     Skip TypeScript build (only Rust + Java)
  --skip-rust   Skip Rust rebuild (only Java + TS)
  --clean       Clear redb cache, aspect dirs, and stale build/ artifacts
  --help        Show this help

Output:
  vscode-extension/server/com.bazel.jdt.jar  (OSGi bundle with embedded native lib)
EOF
            exit 0
            ;;
        *)
            echo "ERROR: Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "=== Bazel JDT Bridge — Debug Build ==="
echo ""

# --- Step 0: Clean caches and stale artifacts ---
if [[ "$CLEAN" == true ]]; then
    echo "--- [clean] Clearing caches and stale artifacts ---"

    cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/bazel-jdt"
    rm -f "$cache_dir/bazel-jdt-cache.redb"
    echo "  Cleared $cache_dir/bazel-jdt-cache.redb"

    for ws in "$PROJECT_ROOT"/../examples/*/; do
        aspect_dir="$ws/.bazel-jdt/aspects"
        if [[ -d "$aspect_dir" ]]; then
            rm -rf "$aspect_dir"
            echo "  Cleared $aspect_dir"
        fi
    done

    if [[ -d "$BUILD_DIR" ]]; then
        rm -rf "$BUILD_DIR"
        echo "  Cleared stale build/ directory"
    fi

    echo ""
fi

# --- Helper: verify key classes in JAR are loadable ---
verify_jar_classes() {
    local jar_path="$1"
    local work_dir
    work_dir="$(mktemp -d)"
    local failed=0

    for cls in "${VERIFY_CLASSES[@]}"; do
        local class_file="${cls//.//}.class"
        if ! jar tf "$jar_path" "$class_file" &>/dev/null; then
            echo "  WARNING: $class_file not found in JAR"
            continue
        fi
        cd "$work_dir"
        if ! jar xf "$jar_path" "$class_file" 2>/dev/null; then
            echo "  WARNING: Failed to extract $class_file"
            continue
        fi
        if ! javap "$cls" &>/dev/null; then
            echo "  ERROR: $cls failed javap verification — ClassFormatError likely"
            failed=1
        fi
    done

    rm -rf "$work_dir"
    cd "$PROJECT_ROOT"
    return $failed
}

# --- Step 1: Rust native library ---
if [[ "$SKIP_RUST" == false ]]; then
    echo "--- [1/3] Building Rust native library ---"
    cd "$PROJECT_ROOT"

    cargo build --release -p bazel-jdt-core

    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"
    case "$os" in
        linux)
            platform_dir="linux-$arch"
            lib_name="libbazel_jdt_core.so"
            ;;
        darwin)
            platform_dir="darwin-$arch"
            lib_name="libbazel_jdt_core.dylib"
            ;;
        msys*|mingw*|cygwin*)
            platform_dir="windows-$arch"
            lib_name="bazel_jdt_core.dll"
            ;;
        *)
            echo "ERROR: Unsupported OS: $os"
            exit 1
            ;;
    esac

    native_dir="$PROJECT_ROOT/java-bridge/src/main/resources/native/$platform_dir"
    mkdir -p "$native_dir"
    cp "$PROJECT_ROOT/target/release/$lib_name" "$native_dir/"
    echo "  Copied $lib_name -> $native_dir/"
    echo ""
fi

# --- Step 2: Java OSGi bundle ---
step_num=$([ "$SKIP_RUST" == false ] && echo "2/3" || echo "1/2")
echo "--- [$step_num] Building Java OSGi bundle ---"
cd "$PROJECT_ROOT/java-bridge"
mvn clean package -DskipTests -q

jar_source=(target/bazel-jdt-bridge-*.jar)
if [[ ${#jar_source[@]} -eq 0 ]]; then
    echo "ERROR: JAR not found in target/"
    exit 1
fi
jar_source="${jar_source[0]}"

server_dir="$PROJECT_ROOT/vscode-extension/server"
mkdir -p "$server_dir"
cp "$jar_source" "$SERVER_JAR"
echo "  Copied $(basename "$jar_source") -> $SERVER_JAR"

native_count=$(jar tf "$SERVER_JAR" | grep -c '^native/.*\.\(so\|dylib\|dll\)$' || true)
echo "  Native libraries in JAR: $native_count"

if command -v javap &>/dev/null; then
    echo "  Verifying class files..."
    if verify_jar_classes "$SERVER_JAR"; then
        echo "  All key classes verified OK"
    else
        echo "  ERROR: Class verification failed. Do NOT deploy this JAR."
        exit 1
    fi
fi

jar_size=$(wc -c < "$SERVER_JAR" | tr -d ' ')
jar_sha=$(sha256sum "$SERVER_JAR" | cut -c1-12)
echo "  JAR: ${jar_size} bytes  SHA: ${jar_sha}..."
echo ""

# --- Step 3: TypeScript extension ---
if [[ "$SKIP_TS" == false ]]; then
    step=$([ "$SKIP_RUST" == false ] && echo "3/3" || echo "2/2")
    echo "--- [$step] Building VS Code extension bundle ---"
    cd "$PROJECT_ROOT/vscode-extension"

    if [[ ! -d node_modules ]]; then
        echo "  Installing npm dependencies..."
        npm install --silent
    fi

    npm run build
    echo "  Built dist/extension.js"
    echo ""
fi

echo "=== Done! ==="
echo ""
echo "To start debugging:"
echo "  1. Open bazel-jdt-bridge/vscode-extension/ in VS Code"
echo "  2. Press F5 (Launch Extension Development Host)"
echo "  3. Open a Bazel workspace with Java targets"
echo ""
echo "Server JAR: $SERVER_JAR (${jar_size} bytes, SHA: ${jar_sha}...)"
