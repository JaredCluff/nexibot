#!/bin/bash
#
# Start NexiBot Bridge Service
#
# This script starts the plugin-based bridge service that enables
# OAuth token support and provider SDK integration for NexiBot.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Check if Node.js is installed
if ! command -v node &> /dev/null; then
    echo "Error: Node.js is not installed"
    echo "Please install Node.js from https://nodejs.org/"
    exit 1
fi

# Check Node.js version (need >=18.0.0)
NODE_VERSION=$(node -v | cut -d'v' -f2)
NODE_MAJOR=$(echo "$NODE_VERSION" | cut -d'.' -f1)

if [ "$NODE_MAJOR" -lt 18 ]; then
    echo "Error: Node.js version $NODE_VERSION is too old"
    echo "Please upgrade to Node.js 18.0.0 or later"
    exit 1
fi

# Install core dependencies if needed
if [ ! -d "node_modules" ]; then
    echo "Installing core dependencies..."
    npm install
fi

# Install plugin dependencies
for plugin_dir in plugins/*/; do
    if [ -f "$plugin_dir/package.json" ] && [ ! -d "$plugin_dir/node_modules" ]; then
        echo "Installing dependencies for $(basename "$plugin_dir")..."
        (cd "$plugin_dir" && npm install)
    fi
done

# Check if bridge is already running
if lsof -Pi :18790 -sTCP:LISTEN -t >/dev/null 2>&1; then
    echo "Warning: Bridge is already running on port 18790"
    echo ""
    read -p "Kill existing process and restart? (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "Killing existing process..."
        lsof -ti:18790 | xargs kill -9 2>/dev/null || true
        sleep 1
    else
        echo "Exiting"
        exit 0
    fi
fi

# Start the bridge
echo "Starting NexiBot Bridge Service..."
echo ""

exec npm start
