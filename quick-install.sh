#!/bin/bash
# quick-install.sh — Fast incremental release build + signed install
#
# Faster than `cargo tauri build` for dev iterations because:
#   - Skips DMG/appimage packaging
#   - Incremental Rust compilation (only changed crates recompile)
#
# Preserves TCC permissions because:
#   - Builds a RELEASE binary (embedded UI, not dev server)
#   - Signs with the same identity as the full build
#   - Re-signs the bundle so macOS recognizes it as the same app
#
# Usage: cd nexibot && ./quick-install.sh [--ui]
#   --ui  Also rebuild the UI (needed if you changed .tsx/.css files)

set -e

SIGNING_IDENTITY="Apple Development: JARED BENJAMIN CLUFF (NFFXB6V5X6)"
ENTITLEMENTS="src-tauri/entitlements.plist"
APP_BUNDLE="/Applications/NexiBot.app"
CONFIG_PATH="$HOME/Library/Application Support/ai.nexibot.desktop/config.yaml"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== NexiBot Quick Install ==="
echo ""

# --- Step 1: Backup config ---
echo "[1/6] Backing up config..."
if [ -f "$CONFIG_PATH" ]; then
    cp "$CONFIG_PATH" "${CONFIG_PATH}.pre-build"
    echo "      Backed up to config.yaml.pre-build"
else
    echo "      WARNING: No config found at $CONFIG_PATH"
fi

# --- Step 2: Build UI note ---
# UI is always rebuilt by cargo tauri build --no-bundle via beforeBuildCommand.
# The --ui flag is kept for compatibility but is now a no-op.
echo "[2/6] UI will be rebuilt by cargo tauri build --no-bundle (beforeBuildCommand)"

# --- Step 3: Build release binary ---
# The Tauri CLI is used (not plain cargo) so TAURI_ENV_DEBUG and other
# required env vars are set correctly, ensuring the UI is embedded from
# frontendDist rather than the dev server URL.
echo "[3/6] Building release binary (incremental)..."
cd "$SCRIPT_DIR"
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo tauri build --no-bundle
echo "      Built: ../target/release/nexibot-tauri"

# --- Step 4: Kill running instances ---
echo "[4/6] Killing running instances..."
osascript -e 'tell application "NexiBot" to quit' 2>/dev/null || true
sleep 1
pkill -9 -f nexibot-tauri 2>/dev/null || true
sleep 1

# --- Step 5: Swap binary + re-sign bundle ---
echo "[5/6] Installing and signing..."
# Binary is at the workspace root target (shared by all workspace members).
# The src-tauri/target/ directory contains only build artifacts, not the binary.
BINARY_PATH="../target/release/nexibot-tauri"
if [ ! -f "$BINARY_PATH" ]; then
    echo "ERROR: Binary not found at $BINARY_PATH"
    exit 1
fi
# Copy new binary into existing bundle (preserves icons, bridge, Info.plist)
cp "$BINARY_PATH" "$APP_BUNDLE/Contents/MacOS/nexibot-tauri"
chmod +x "$APP_BUNDLE/Contents/MacOS/nexibot-tauri"

# Re-sign the entire bundle with same identity + entitlements
# --force: replace existing signature
# --deep: sign nested frameworks/dylibs too
codesign \
    --sign "$SIGNING_IDENTITY" \
    --entitlements "$SCRIPT_DIR/$ENTITLEMENTS" \
    --force \
    --deep \
    --options runtime \
    "$APP_BUNDLE"

echo "      Signed: $APP_BUNDLE"

# --- Step 6: Purge stale Launch Services entries ---
echo "[6/7] Purging stale Launch Services entries..."
LSREG="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
"$LSREG" -dump 2>/dev/null \
    | grep -i "nexibot" \
    | grep "path:" \
    | grep -v "path:.*Applications" \
    | sed 's/.*path:[[:space:]]*//' \
    | sed 's/ (0x[0-9a-f]*)$//' \
    | sort -u \
    | while IFS= read -r path; do "$LSREG" -u "$path" 2>/dev/null; done
"$LSREG" "$APP_BUNDLE" 2>/dev/null
killall Dock
sleep 1
echo "      Launch Services cleaned, Dock restarted"

# --- Step 7: Launch and verify ---
echo "[7/7] Launching..."
open "$APP_BUNDLE"
sleep 3

PID=$(pgrep -f nexibot-tauri 2>/dev/null | head -1)
if [ -n "$PID" ]; then
    echo ""
    echo "=== SUCCESS ==="
    echo "NexiBot running (PID $PID)"
    echo ""
    echo "Verify manually:"
    echo "  [x] Menubar brain icon visible in macOS menubar"
    echo "  [x] Clicking icon opens UI with content (not blank)"
    echo "  [ ] TCC permissions preserved (no speech/mic prompts)"
else
    echo ""
    echo "=== FAILED — Process not running after launch ==="
    echo "Check: log show --predicate 'process CONTAINS \"nexibot\"' --last 1m"
    exit 1
fi
