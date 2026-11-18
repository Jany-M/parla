#!/usr/bin/env bash
# Builds the Parla universal macOS DMG (Intel + Apple Silicon).
# Must run on macOS. Windows uses scripts/build.ps1 instead.
#
# Usage:
#   ./scripts/build-macos.sh              # npm ci + tests + dmg
#   ./scripts/build-macos.sh --skip-tests
#   ./scripts/build-macos.sh --skip-npm --skip-tests
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SKIP_TESTS=0
SKIP_NPM=0
for arg in "$@"; do
  case "$arg" in
    --skip-tests) SKIP_TESTS=1 ;;
    --skip-npm) SKIP_NPM=1 ;;
    -h|--help)
      sed -n '2,9p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-macos.sh only runs on macOS (detected: $(uname -s)). Use scripts/build.ps1 on Windows." >&2
  exit 1
fi

step() { printf '\n==> %s\n' "$1"; }

step "Checking prerequisites"
command -v cargo >/dev/null || { echo "Rust (cargo) not found. Install from https://rustup.rs" >&2; exit 1; }
command -v node >/dev/null || { echo "Node.js not found (need 22+)." >&2; exit 1; }
command -v xcodebuild >/dev/null || { echo "Xcode Command Line Tools not found. Run: xcode-select --install" >&2; exit 1; }

step "Adding Apple Darwin Rust targets"
rustup target add aarch64-apple-darwin x86_64-apple-darwin

if [[ "$SKIP_NPM" -eq 1 && -d node_modules ]]; then
  step "Skipping npm install (node_modules already present)"
else
  step "Installing JavaScript dependencies"
  npm ci
fi

if [[ "$SKIP_TESTS" -eq 0 ]]; then
  step "Rust tests (parla-core)"
  cargo test -p parla-core --lib
  step "Frontend type-check and tests"
  npm run typecheck
  npm test
fi

step "Building universal macOS DMG"
npm run tauri build -- --target universal-apple-darwin

BUNDLE_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/universal-apple-darwin/release/bundle/dmg"
if [[ ! -d "$BUNDLE_DIR" ]]; then
  # Fallback layout some Tauri versions use under target/release when not cross-linking.
  BUNDLE_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/release/bundle/dmg"
fi
DMG="$(ls -t "$BUNDLE_DIR"/*.dmg 2>/dev/null | head -n 1 || true)"
if [[ -z "$DMG" || ! -f "$DMG" ]]; then
  echo "Build finished but no DMG found under $BUNDLE_DIR" >&2
  exit 1
fi

OUT="$ROOT/release"
mkdir -p "$OUT"
cp -f "$DMG" "$OUT/"
echo
echo "Done: $OUT/$(basename "$DMG")"
echo "Unsigned: first open may need right-click → Open, or: xattr -dr com.apple.quarantine /Applications/Parla.app"
