#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════
# NexiBot Test Orchestrator
# ═══════════════════════════════════════════════════════════════════════════
#
# Runs all test tiers in sequence. Each tier can be run independently.
#
# Usage:
#   ./run-tests.sh              # Run all tiers
#   ./run-tests.sh unit         # Rust + UI unit tests only
#   ./run-tests.sh e2e          # Playwright E2E only
#   ./run-tests.sh integration  # API smoke + channel simulators
#   ./run-tests.sh mock-llm     # Start mock LLM server (foreground)
#
# Prerequisites:
#   - Node.js 20+
#   - Rust toolchain
#   - Playwright browsers: cd tests/e2e && npx playwright install chromium
#
# Environment:
#   MOCK_PORT=18799              Mock LLM server port
#   NEXIBOT_API_URL=...          NexiBot API server URL (for integration)
#   ANTHROPIC_API_KEY=...        For recording new fixtures
#   OPENAI_API_KEY=...           For recording new fixtures
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MOCK_PORT="${MOCK_PORT:-18799}"
MOCK_PID=""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_header() { echo -e "\n${BLUE}═══ $1 ═══${NC}\n"; }
log_pass()   { echo -e "${GREEN}PASS${NC}: $1"; }
log_fail()   { echo -e "${RED}FAIL${NC}: $1"; }
log_info()   { echo -e "${YELLOW}INFO${NC}: $1"; }

# ─── Mock LLM Server ────────────────────────────────────────────────────────

start_mock_llm() {
  log_header "Starting Mock LLM Server"
  cd "$SCRIPT_DIR/mock-llm"
  npm install --silent 2>/dev/null

  MOCK_MODE="${MOCK_MODE:-auto}" MOCK_PORT="$MOCK_PORT" node server.js &
  MOCK_PID=$!
  sleep 2

  # Health check
  if curl -sf "http://127.0.0.1:$MOCK_PORT/health" > /dev/null 2>&1; then
    log_pass "Mock LLM running on :$MOCK_PORT (PID: $MOCK_PID)"
  else
    log_fail "Mock LLM failed to start"
    exit 1
  fi
}

stop_mock_llm() {
  if [ -n "$MOCK_PID" ] && kill -0 "$MOCK_PID" 2>/dev/null; then
    log_info "Stopping Mock LLM (PID: $MOCK_PID)"
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
}

trap stop_mock_llm EXIT

# ─── Tier 1: Unit Tests ─────────────────────────────────────────────────────

run_unit_tests() {
  log_header "Tier 1: Unit Tests"

  log_info "Running Rust unit tests..."
  cd "$PROJECT_DIR/src-tauri"
  CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --lib -p nexibot-tauri -- --test-threads=1 2>&1 | tail -20
  RUST_EXIT=${PIPESTATUS[0]:-$?}
  if [ "$RUST_EXIT" -eq 0 ]; then
    log_pass "Rust unit tests"
  else
    log_fail "Rust unit tests (exit code: $RUST_EXIT)"
  fi

  log_info "Running UI unit tests..."
  cd "$PROJECT_DIR/ui"
  npm run test 2>&1 | tail -20
  UI_EXIT=${PIPESTATUS[0]:-$?}
  if [ "$UI_EXIT" -eq 0 ]; then
    log_pass "UI unit tests (vitest)"
  else
    log_fail "UI unit tests (exit code: $UI_EXIT)"
  fi
}

# ─── Tier 2: E2E Tests ──────────────────────────────────────────────────────

run_e2e_tests() {
  log_header "Tier 2: E2E Tests (Playwright)"

  cd "$SCRIPT_DIR/e2e"
  npm install --silent 2>/dev/null
  npx playwright install chromium --with-deps 2>/dev/null

  # E2E tests run against Vite dev server with Tauri mock
  npx playwright test 2>&1 | tail -30
  E2E_EXIT=${PIPESTATUS[0]:-$?}
  if [ "$E2E_EXIT" -eq 0 ]; then
    log_pass "Playwright E2E tests"
  else
    log_fail "Playwright E2E tests (exit code: $E2E_EXIT)"
  fi
}

# ─── Tier 3: API Smoke Tests ────────────────────────────────────────────────

run_api_smoke() {
  log_header "Tier 3: API Smoke Tests"

  cd "$SCRIPT_DIR/channels"

  log_info "Running API smoke tests..."
  node api-smoke.js 2>&1
  SMOKE_EXIT=$?
  if [ "$SMOKE_EXIT" -eq 0 ]; then
    log_pass "API smoke tests"
  else
    log_fail "API smoke tests (exit code: $SMOKE_EXIT)"
  fi
}

# ─── Tier 4: Channel Simulators ─────────────────────────────────────────────

run_channel_sims() {
  log_header "Tier 4: Channel Simulators"

  cd "$SCRIPT_DIR/channels"

  log_info "Running Telegram simulator..."
  node telegram-sim.js 2>&1 || true

  log_info "Running WhatsApp simulator..."
  node whatsapp-sim.js 2>&1 || true
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
  local tier="${1:-all}"

  echo -e "${BLUE}"
  echo "╔══════════════════════════════════════════════════════════════╗"
  echo "║             NexiBot Test Orchestrator                       ║"
  echo "╚══════════════════════════════════════════════════════════════╝"
  echo -e "${NC}"

  case "$tier" in
    unit)
      run_unit_tests
      ;;
    e2e)
      run_e2e_tests
      ;;
    integration)
      run_api_smoke
      run_channel_sims
      ;;
    mock-llm)
      start_mock_llm
      log_info "Mock LLM running in foreground. Ctrl+C to stop."
      wait "$MOCK_PID"
      ;;
    all)
      run_unit_tests
      run_e2e_tests
      run_api_smoke
      # Channel sims require running NexiBot — skip if not reachable
      if curl -sf "http://127.0.0.1:18791/webhook/health" > /dev/null 2>&1; then
        run_channel_sims
      else
        log_info "Skipping channel simulators (NexiBot webhook server not running)"
      fi
      ;;
    *)
      echo "Usage: $0 {unit|e2e|integration|mock-llm|all}"
      exit 1
      ;;
  esac

  log_header "Done"
}

main "$@"
