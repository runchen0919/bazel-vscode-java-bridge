#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build"

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"

echo "--- Building Java bridge JAR ---"
cd "$PROJECT_ROOT/java-bridge"
mvn clean package -DskipTests
cp target/bazel-jdt-bridge-*.jar "$BUILD_DIR/com.bazel.jdt.jar"

echo "--- Building VSCode extension ---"
cd "$PROJECT_ROOT/vscode-extension"
npm install
npm run build

mkdir -p "$BUILD_DIR/vscode-extension/server"
cp "$BUILD_DIR/com.bazel.jdt.jar" "$BUILD_DIR/vscode-extension/server/"
cp package.json "$BUILD_DIR/vscode-extension/"
cp -r dist "$BUILD_DIR/vscode-extension/"

cd "$BUILD_DIR/vscode-extension"
npx @vscode/vsce package --no-dependencies
mv *.vsix "$BUILD_DIR/"

echo "Done: $BUILD_DIR/"
ls -la "$BUILD_DIR/"
