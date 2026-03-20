/**
 * API Server Smoke Tests
 *
 * Tests the NexiBot HTTP API endpoints for basic functionality.
 * These endpoints are used by mobile clients and the E2E test harness.
 *
 * Usage:
 *   API_URL=http://127.0.0.1:11434 AUTH_TOKEN=your-token node api-smoke.js
 */

const API_URL = process.env.API_URL || 'http://127.0.0.1:11434';
const AUTH_TOKEN = process.env.AUTH_TOKEN || '';

let testsPassed = 0;
let testsFailed = 0;

function assert(condition, message) {
  if (condition) {
    console.log(`  PASS: ${message}`);
    testsPassed++;
  } else {
    console.log(`  FAIL: ${message}`);
    testsFailed++;
  }
}

async function apiGet(path, description) {
  const url = `${API_URL}${path}`;
  console.log(`\n[TEST] ${description}`);
  console.log(`  GET ${url}`);

  try {
    const headers = { 'Content-Type': 'application/json' };
    if (AUTH_TOKEN) headers['Authorization'] = `Bearer ${AUTH_TOKEN}`;

    const response = await fetch(url, { headers });
    const status = response.status;
    const body = await response.text();
    console.log(`  Status: ${status}`);
    console.log(`  Body: ${body.substring(0, 300)}`);
    return { status, body, json: status === 200 ? JSON.parse(body) : null };
  } catch (err) {
    console.log(`  ERROR: ${err.message}`);
    return { status: 0, body: '', json: null, error: err.message };
  }
}

async function apiPost(path, data, description) {
  const url = `${API_URL}${path}`;
  console.log(`\n[TEST] ${description}`);
  console.log(`  POST ${url}`);

  try {
    const headers = { 'Content-Type': 'application/json' };
    if (AUTH_TOKEN) headers['Authorization'] = `Bearer ${AUTH_TOKEN}`;

    const response = await fetch(url, {
      method: 'POST',
      headers,
      body: JSON.stringify(data),
    });
    const status = response.status;
    const body = await response.text();
    console.log(`  Status: ${status}`);
    console.log(`  Body: ${body.substring(0, 300)}`);
    return { status, body, json: status === 200 ? JSON.parse(body) : null };
  } catch (err) {
    console.log(`  ERROR: ${err.message}`);
    return { status: 0, body: '', json: null, error: err.message };
  }
}

// ─── Test Cases ─────────────────────────────────────────────────────────────

async function testHealth() {
  const result = await apiGet('/api/health', 'Health endpoint (no auth required)');
  assert(result.status === 200, 'Returns 200 OK');
  if (result.json) {
    assert(result.json.status === 'ok', 'Status is "ok"');
  }
}

async function testAuthRequired() {
  // Try without auth token
  const url = `${API_URL}/api/config`;
  console.log(`\n[TEST] Auth required for protected endpoints`);
  const response = await fetch(url);
  const status = response.status;
  console.log(`  Status: ${status}`);
  assert(status === 401 || status === 403, 'Returns 401/403 without auth');
}

async function testGetConfig() {
  const result = await apiGet('/api/config', 'Get configuration');
  if (result.status === 401) {
    console.log('  SKIP: Auth token not configured');
    return;
  }
  assert(result.status === 200, 'Returns 200 OK');
  if (result.json) {
    assert(typeof result.json.claude === 'object', 'Config has claude section');
  }
}

async function testListSessions() {
  const result = await apiGet('/api/sessions', 'List sessions');
  if (result.status === 401) {
    console.log('  SKIP: Auth token not configured');
    return;
  }
  assert(result.status === 200, 'Returns 200 OK');
  if (result.json) {
    assert(Array.isArray(result.json), 'Returns an array');
  }
}

async function testListModels() {
  const result = await apiGet('/api/models', 'List available models');
  if (result.status === 401) {
    console.log('  SKIP: Auth token not configured');
    return;
  }
  assert(result.status === 200, 'Returns 200 OK');
}

async function testListSkills() {
  const result = await apiGet('/api/skills', 'List loaded skills');
  if (result.status === 401) {
    console.log('  SKIP: Auth token not configured');
    return;
  }
  assert(result.status === 200, 'Returns 200 OK');
}

async function testChatSend() {
  const result = await apiPost('/api/chat/send', {
    message: 'Hello from API smoke test!',
  }, 'Send chat message');
  if (result.status === 401) {
    console.log('  SKIP: Auth token not configured');
    return;
  }
  // Should return 200 with a response, or 500 if no LLM configured
  assert(result.status === 200 || result.status === 500, 'Returns 200 or 500 (no LLM)');
}

async function testGetOverrides() {
  const result = await apiGet('/api/overrides', 'Get session overrides');
  if (result.status === 401) {
    console.log('  SKIP: Auth token not configured');
    return;
  }
  assert(result.status === 200, 'Returns 200 OK');
}

// ─── Run All ────────────────────────────────────────────────────────────────

async function main() {
  console.log('═══════════════════════════════════════════════');
  console.log(' API Server Smoke Tests');
  console.log('═══════════════════════════════════════════════');
  console.log(`Target: ${API_URL}`);
  console.log(`Auth: ${AUTH_TOKEN ? 'configured' : 'NOT SET (some tests will skip)'}`);

  // Check reachability
  try {
    await fetch(`${API_URL}/api/health`);
  } catch {
    console.error(`\nERROR: Cannot reach ${API_URL}. Is NexiBot running with the API server enabled?`);
    process.exit(1);
  }

  await testHealth();
  await testAuthRequired();
  await testGetConfig();
  await testListSessions();
  await testListModels();
  await testListSkills();
  await testChatSend();
  await testGetOverrides();

  console.log('\n═══════════════════════════════════════════════');
  console.log(` Results: ${testsPassed} passed, ${testsFailed} failed`);
  console.log('═══════════════════════════════════════════════');
  process.exit(testsFailed > 0 ? 1 : 0);
}

main();
