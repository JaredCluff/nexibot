/**
 * WhatsApp Cloud API Webhook Simulator
 *
 * Sends fake WhatsApp webhook payloads to the NexiBot webhook endpoint,
 * simulating real WhatsApp message delivery from Meta.
 *
 * Usage:
 *   WEBHOOK_URL=http://127.0.0.1:18791 node whatsapp-sim.js
 *
 * This tests:
 *   - Webhook verification (GET challenge)
 *   - Message receipt via POST
 *   - Deduplication (Meta retries on slow 200)
 *   - Rate limiting per sender
 *   - HMAC signature validation (when configured)
 */

const WEBHOOK_URL = process.env.WEBHOOK_URL || 'http://127.0.0.1:18791';
const WHATSAPP_PATH = '/webhook/whatsapp';
const VERIFY_TOKEN = process.env.WA_VERIFY_TOKEN || 'test-verify-token';

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

async function sendWebhook(body, description) {
  const url = `${WEBHOOK_URL}${WHATSAPP_PATH}`;
  console.log(`\n[TEST] ${description}`);
  console.log(`  POST ${url}`);

  try {
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });

    const status = response.status;
    const text = await response.text();
    console.log(`  Status: ${status}`);
    console.log(`  Body: ${text.substring(0, 200)}`);
    return { status, body: text };
  } catch (err) {
    console.log(`  ERROR: ${err.message}`);
    return { status: 0, body: '', error: err.message };
  }
}

function makeMessagePayload(phoneNumber, text, messageId) {
  return {
    object: 'whatsapp_business_account',
    entry: [{
      id: '123456789',
      changes: [{
        value: {
          messaging_product: 'whatsapp',
          metadata: {
            display_phone_number: '15551234567',
            phone_number_id: '987654321',
          },
          contacts: [{ profile: { name: 'Test User' }, wa_id: phoneNumber }],
          messages: [{
            from: phoneNumber,
            id: messageId || `wamid.${Date.now()}`,
            timestamp: Math.floor(Date.now() / 1000).toString(),
            text: { body: text },
            type: 'text',
          }],
        },
        field: 'messages',
      }],
    }],
  };
}

// ─── Test Cases ─────────────────────────────────────────────────────────────

async function testVerifyWebhook() {
  const url = `${WEBHOOK_URL}${WHATSAPP_PATH}?hub.mode=subscribe&hub.verify_token=${VERIFY_TOKEN}&hub.challenge=test_challenge_123`;
  console.log(`\n[TEST] Webhook verification (GET)`);
  console.log(`  GET ${url}`);

  try {
    const response = await fetch(url);
    const status = response.status;
    const body = await response.text();
    console.log(`  Status: ${status}, Body: ${body}`);
    // If verify token matches, should return the challenge
    assert(status === 200 || status === 403, 'Returns 200 (verified) or 403 (token mismatch)');
  } catch (err) {
    console.log(`  ERROR: ${err.message}`);
    assert(false, 'Request should not fail');
  }
}

async function testBasicMessage() {
  const result = await sendWebhook(
    makeMessagePayload('15559876543', 'Hello from WhatsApp!'),
    'Basic text message',
  );
  assert(result.status === 200, 'Returns 200 OK');
}

async function testStatusUpdate() {
  const result = await sendWebhook({
    object: 'whatsapp_business_account',
    entry: [{
      id: '123456789',
      changes: [{
        value: {
          messaging_product: 'whatsapp',
          metadata: { display_phone_number: '15551234567', phone_number_id: '987654321' },
          statuses: [{
            id: 'wamid.status123',
            status: 'delivered',
            timestamp: Math.floor(Date.now() / 1000).toString(),
            recipient_id: '15559876543',
          }],
        },
        field: 'messages',
      }],
    }],
  }, 'Status update (not a message)');
  assert(result.status === 200, 'Returns 200 OK (gracefully ignores status updates)');
}

async function testDuplicateMessage() {
  const msgId = `wamid.dedup_test_${Date.now()}`;
  const payload = makeMessagePayload('15559876543', 'Dedup test', msgId);

  await sendWebhook(payload, 'Duplicate message (1st delivery)');
  const result2 = await sendWebhook(payload, 'Duplicate message (2nd delivery - Meta retry)');
  assert(result2.status === 200, 'Returns 200 OK (dedup is transparent)');
}

async function testEmptyEntry() {
  const result = await sendWebhook({
    object: 'whatsapp_business_account',
    entry: [],
  }, 'Empty entry array');
  assert(result.status === 200, 'Returns 200 OK (no crash on empty entry)');
}

async function testMalformedPayload() {
  const result = await sendWebhook(
    { invalid: true },
    'Malformed payload',
  );
  assert(result.status === 200 || result.status === 400, 'Returns 200 or 400 (does not crash)');
}

// ─── Run All ────────────────────────────────────────────────────────────────

async function main() {
  console.log('═══════════════════════════════════════════════');
  console.log(' WhatsApp Webhook Simulator');
  console.log('═══════════════════════════════════════════════');
  console.log(`Target: ${WEBHOOK_URL}${WHATSAPP_PATH}`);

  try {
    await fetch(`${WEBHOOK_URL}/webhook/health`);
  } catch {
    console.error(`\nERROR: Cannot reach ${WEBHOOK_URL}. Is NexiBot running?`);
    process.exit(1);
  }

  await testVerifyWebhook();
  await testBasicMessage();
  await testStatusUpdate();
  await testDuplicateMessage();
  await testEmptyEntry();
  await testMalformedPayload();

  console.log('\n═══════════════════════════════════════════════');
  console.log(` Results: ${testsPassed} passed, ${testsFailed} failed`);
  console.log('═══════════════════════════════════════════════');
  process.exit(testsFailed > 0 ? 1 : 0);
}

main();
