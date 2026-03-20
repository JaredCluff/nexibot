#!/bin/bash
#
# Test Anthropic Bridge Service
#
# This script tests the bridge service with a simple health check
# and optionally a test message.

set -e

BRIDGE_URL="${ANTHROPIC_BRIDGE_URL:-http://127.0.0.1:18790}"

echo "🧪 Testing Anthropic Bridge Service"
echo "Bridge URL: $BRIDGE_URL"
echo ""

# Test 1: Health check
echo "Test 1: Health check..."
if curl -s -f "$BRIDGE_URL/health" > /dev/null; then
    echo "✅ Bridge is healthy"
    curl -s "$BRIDGE_URL/health" | jq .
else
    echo "❌ Bridge is not responding"
    echo "Make sure the bridge is running: ./start-bridge.sh"
    exit 1
fi

echo ""

# Test 2: Test message (only if API key is provided)
if [ -n "$ANTHROPIC_API_KEY" ]; then
    echo "Test 2: Sending test message..."

    RESPONSE=$(curl -s -X POST "$BRIDGE_URL/api/messages" \
        -H "Content-Type: application/json" \
        -d "{
            \"apiKey\": \"$ANTHROPIC_API_KEY\",
            \"model\": \"claude-sonnet-4-5-20250929\",
            \"max_tokens\": 100,
            \"messages\": [
                {
                    \"role\": \"user\",
                    \"content\": \"Say 'Bridge test successful!' and nothing else.\"
                }
            ]
        }")

    if echo "$RESPONSE" | jq -e '.content[0].text' > /dev/null 2>&1; then
        echo "✅ Bridge responded successfully"
        echo "Response:"
        echo "$RESPONSE" | jq -r '.content[0].text'
    else
        echo "❌ Bridge returned an error"
        echo "$RESPONSE" | jq .
        exit 1
    fi
else
    echo "ℹ️  Skipping test message (set ANTHROPIC_API_KEY to test)"
fi

echo ""
echo "✅ All tests passed!"
