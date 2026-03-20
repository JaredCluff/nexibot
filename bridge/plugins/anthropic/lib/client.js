/**
 * Anthropic SDK client creation with OAuth token detection.
 */

import Anthropic from '@anthropic-ai/sdk';

/**
 * Check if an API key is an OAuth token.
 */
export function isOAuthToken(apiKey) {
  return apiKey && apiKey.includes('sk-ant-oat');
}

/**
 * Create Anthropic client with proper OAuth handling.
 *
 * OAuth tokens use the `authToken` parameter and mimic Claude Code headers.
 * Regular API keys use the standard `apiKey` parameter.
 */
export function createClient(apiKey, { keyFingerprint } = {}) {
  const isOAuth = isOAuthToken(apiKey);

  if (isOAuth) {
    console.log('[Bridge:Anthropic] Creating OAuth client');
    if (keyFingerprint) {
      console.log('[Bridge:Anthropic] Token fingerprint (sha256):', keyFingerprint(apiKey));
    }

    // Mimic Claude Code headers exactly (from OpenClaw/pi-ai)
    const defaultHeaders = {
      'accept': 'application/json',
      'anthropic-dangerous-direct-browser-access': 'true',
      'anthropic-beta': 'claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14',
      'user-agent': 'claude-cli/2.1.2 (external, cli)',
      'x-app': 'cli',
    };

    // CRITICAL: Use authToken parameter for OAuth, NOT apiKey
    return new Anthropic({
      apiKey: null,
      authToken: apiKey,
      defaultHeaders,
      dangerouslyAllowBrowser: true,
      timeout: 600_000, // 10 min — matches OpenClaw's DEFAULT_AGENT_TIMEOUT_SECONDS
    });
  }

  console.log('[Bridge:Anthropic] Creating API key client');
  if (keyFingerprint) {
    console.log('[Bridge:Anthropic] API key fingerprint (sha256):', keyFingerprint(apiKey));
  }

  // Regular API key client
  return new Anthropic({
    apiKey: apiKey,
    dangerouslyAllowBrowser: true,
    timeout: 600_000, // 10 min — matches OpenClaw's DEFAULT_AGENT_TIMEOUT_SECONDS
  });
}
