#!/usr/bin/env bash
# Builds the Parla universal macOS DMG (Intel + Apple Silicon).
# Must run on macOS. Windows uses scripts/build.ps1 instead.
#
# Usage:
#   ./scripts/build-macos.sh              # npm ci + tests + dmg
#   ./scripts/build-macos.sh --skip-tests
#   ./scripts/build-macos.sh --skip-npm --skip-tests
#
# Missing Rust / Node / Xcode CLT are installed automatically when possible.
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
      sed -n '2,11p' "$0"
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

# Load cargo into this shell after a fresh rustup install (or prior install not on PATH).
load_cargo_env() {
  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
  fi
  export PATH="$HOME/.cargo/bin:$PATH"
}

node_major() {
  node -p "process.versions.node.split('.')[0]" 2>/dev/null || echo 0
}

ensure_xcode_clt() {
  if xcode-select -p >/dev/null 2>&1 && command -v xcodebuild >/dev/null 2>&1; then
    return 0
  fi
  step "Installing Xcode Command Line Tools (GUI prompt may appear; wait until it finishes)"
  # Touch the receipt so softwareupdate lists the CLT package on recent macOS.
  touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress 2>/dev/null || true
  local label
  label="$(softwareupdate -l 2>/dev/null | awk -F'*' '/Command Line Tools/{print $2}' | sed -e 's/^ *Label: //' -e 's/^ *//' | head -n 1 || true)"
  if [[ -n "$label" ]]; then
    # May prompt for an admin password once.
    sudo softwareupdate -i "$label" --agree-to-license || softwareupdate -i "$label" --agree-to-license || true
  fi
  rm -f /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress 2>/dev/null || true
  if xcode-select -p >/dev/null 2>&1 && command -v xcodebuild >/dev/null 2>&1; then
    return 0
  fi
  # Fallback: opens the standard Install dialog (needs a click in System Settings / Install).
  xcode-select --install 2>/dev/null || true
  echo "Waiting for Xcode Command Line Tools (check the Install dialog if one appeared)..."
  local i=0
  while ! xcode-select -p >/dev/null 2>&1; do
    i=$((i + 1))
    if [[ $i -gt 120 ]]; then
      echo "Xcode Command Line Tools are still missing. Finish the Install dialog, then re-run ./scripts/build-macos.sh" >&2
      exit 1
    fi
    sleep 5
  done
  command -v xcodebuild >/dev/null 2>&1 || {
    echo "xcodebuild still not on PATH after CLT install. Open a new terminal and re-run." >&2
    exit 1
  }
}

ensure_rust() {
  load_cargo_env
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    return 0
  fi
  step "Installing Rust (rustup, default stable toolchain)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  load_cargo_env
  command -v cargo >/dev/null 2>&1 || {
    echo "Rust installed but cargo is not on PATH. Open a new terminal and re-run ./scripts/build-macos.sh" >&2
    exit 1
  }
}

ensure_homebrew() {
  if command -v brew >/dev/null 2>&1; then
    return 0
  fi
  # Apple Silicon brew lives here even when not yet on PATH.
  if [[ -x /opt/homebrew/bin/brew ]]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
    return 0
  fi
  if [[ -x /usr/local/bin/brew ]]; then
    eval "$(/usr/local/bin/brew shellenv)"
    return 0
  fi
  step "Installing Homebrew (needed for Node.js)"
  NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  if [[ -x /opt/homebrew/bin/brew ]]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
  elif [[ -x /usr/local/bin/brew ]]; then
    eval "$(/usr/local/bin/brew shellenv)"
  fi
  command -v brew >/dev/null 2>&1 || {
    echo "Homebrew install finished but brew is not on PATH. Open a new terminal and re-run." >&2
    exit 1
  }
}

ensure_node() {
  if command -v node >/dev/null 2>&1 && [[ "$(node_major)" -ge 22 ]]; then
    return 0
  fi
  if command -v node >/dev/null 2>&1; then
    echo "Node.js $(node -v) is too old (need 22+); upgrading via Homebrew."
  fi
  ensure_homebrew
  step "Installing Node.js 22+ (Homebrew)"
  # Prefer current Homebrew node (tracks LTS/current); fall back to node@22 if the formula splits.
  if brew info node >/dev/null 2>&1; then
    brew install node
  else
    brew install node@22
    brew link --force --overwrite node@22 2>/dev/null || true
  fi
  # Refresh PATH for Apple Silicon prefixes.
  [[ -x /opt/homebrew/bin/brew ]] && eval "$(/opt/homebrew/bin/brew shellenv)"
  [[ -x /usr/local/bin/brew ]] && eval "$(/usr/local/bin/brew shellenv)"
  command -v node >/dev/null 2>&1 || {
    echo "Node.js installed but node is not on PATH. Open a new terminal and re-run." >&2
    exit 1
  }
  if [[ "$(node_major)" -lt 22 ]]; then
    echo "Node.js $(node -v) is still below 22 after install." >&2
    exit 1
  fi
}

step "Checking prerequisites"
ensure_xcode_clt
ensure_rust
ensure_node
echo "OK: $(rustc -V 2>/dev/null || true); $(node -v); Xcode CLT present"

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
