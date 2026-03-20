/**
 * Telegram Bot Webhook Simulator
 *
 * Sends fake Telegram update payloads to the NexiBot webhook endpoint,
 * simulating real Telegram message delivery.
 *
 * Usage:
 *   WEBHOOK_URL=http://127.0.0.1:18791 node telegram-sim.js
 *
 * This tests:
 *   - Webhook receipt and parsing
 *   - Message routing through the pipeline
 *   - Response generation (via mock LLM)
 *   - Rate limiting / deduplication (when sending duplicates)
 */

const WEBHOOK_URL = process.env.WEBHOOK_URL || 'http://127.0.0.1:18791';
const TELEGRAM_PATH = '/webhook/telegram';

let testsPassed = 0;
let testsFailed = 0;

async function sendUpdate(update, description) {
  const url = `${WEBHOOK_URL}${TELEGRAM_PATH}`;
  console.log(`\n[TEST] ${description}`);
  console.log(`  POST ${url}`);

  try {
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(update),
    });

    const status = response.status;
    const body = await response.text();
    console.log(`  Status: ${status}`);
    console.log(`  Body: ${body.substring(0, 200)}`);

    return { status, body };
  } catch (err) {
    console.log(`  ERROR: ${err.message}`);
    return { status: 0, body: '', error: err.message };
  }
}

function assert(condition, message) {
  if (condition) {
    console.log(`  PASS: ${message}`);
    testsPassed++;
  } else {
    console.log(`  FAIL: ${message}`);
    testsFailed++;
  }
}

// ─── Test Cases ─────────────────────────────────────────────────────────────

async function testBasicMessage() {
  const result = await sendUpdate({
    update_id: 100001,
    message: {
      message_id: 1,
      from: { id: 12345, is_bot: false, first_name: 'Test', username: 'testuser' },
      chat: { id: 12345, type: 'private', first_name: 'Test', username: 'testuser' },
      date: Math.floor(Date.now() / 1000),
      text: 'Hello NexiBot!',
    },
  }, 'Basic text message');
  assert(result.status === 200, 'Returns 200 OK');
}

async function testEmptyMessage() {
  const result = await sendUpdate({
    update_id: 100002,
    message: {
      message_id: 2,
      from: { id: 12345, is_bot: false, first_name: 'Test' },
      chat: { id: 12345, type: 'private' },
      date: Math.floor(Date.now() / 1000),
      // No text field
    },
  }, 'Message with no text');
  assert(result.status === 200, 'Returns 200 OK (gracefully ignores non-text)');
}

async function testCommandMessage() {
  const result = await sendUpdate({
    update_id: 100003,
    message: {
      message_id: 3,
      from: { id: 12345, is_bot: false, first_name: 'Test' },
      chat: { id: 12345, type: 'private' },
      date: Math.floor(Date.now() / 1000),
      text: '/new',
      entities: [{ offset: 0, length: 4, type: 'bot_command' }],
    },
  }, '/new command');
  assert(result.status === 200, 'Returns 200 OK for /new command');
}

async function testDuplicateMessages() {
  const update = {
    update_id: 100004,
    message: {
      message_id: 999,
      from: { id: 12345, is_bot: false, first_name: 'Test' },
      chat: { id: 12345, type: 'private' },
      date: Math.floor(Date.now() / 1000),
      text: 'Duplicate test',
    },
  };

  await sendUpdate(update, 'Duplicate message (1st send)');
  const result2 = await sendUpdate(update, 'Duplicate message (2nd send - should be deduped)');
  assert(result2.status === 200, 'Returns 200 OK (dedup is transparent)');
}

async function testGroupMessage() {
  const result = await sendUpdate({
    update_id: 100005,
    message: {
      message_id: 5,
      from: { id: 12345, is_bot: false, first_name: 'Test' },
      chat: { id: -100123456, type: 'group', title: 'Test Group' },
      date: Math.floor(Date.now() / 1000),
      text: '@nexibot Hello from group',
    },
  }, 'Group message (mention)');
  assert(result.status === 200, 'Returns 200 OK for group message');
}

async function testMalformedPayload() {
  const result = await sendUpdate({ invalid: true }, 'Malformed update payload');
  assert(result.status === 200 || result.status === 400, 'Returns 200 or 400 (does not crash)');
}

// ─── Run All ────────────────────────────────────────────────────────────────

async function main() {
  console.log('═══════════════════════════════════════════════');
  console.log(' Telegram Webhook Simulator');
  console.log(`═══════════════════════════════════════════════`);
  console.log(`Target: ${WEBHOOK_URL}${TELEGRAM_PATH}`);

  // Check if server is reachable
  try {
    await fetch(`${WEBHOOK_URL}/webhook/health`);
  } catch {
    console.error(`\nERROR: Cannot reach ${WEBHOOK_URL}. Is NexiBot running?`);
    process.exit(1);
  }

  await testBasicMessage();
  await testEmptyMessage();
  await testCommandMessage();
  await testDuplicateMessages();
  await testGroupMessage();
  await testMalformedPayload();

  console.log('\n═══════════════════════════════════════════════');
  console.log(` Results: ${testsPassed} passed, ${testsFailed} failed`);
  console.log('═══════════════════════════════════════════════');
  process.exit(testsFailed > 0 ? 1 : 0);
}

main();
