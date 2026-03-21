/**
 * Inference logging middleware for the NexiBot bridge.
 *
 * Logs all LLM API calls (model, token counts, latency) to a JSONL file
 * for observability and debugging.
 */

import { appendFileSync, mkdirSync, existsSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';

// Validate LOG_DIR doesn't contain path traversal sequences
function validateLogDir(dir) {
  if (dir.includes('..')) {
    console.warn('[INFERENCE_LOG] LOG_DIR contains path traversal, using default');
    return join(homedir(), '.config', 'nexibot', 'logs');
  }
  return dir;
}

// Sanitize error messages to remove potential API key leaks
function sanitizeError(msg) {
  if (!msg) return msg;
  return msg
    .replace(/\b(sk-[a-zA-Z0-9]{20,})\b/g, 'sk-***REDACTED***')
    .replace(/\b(key-[a-zA-Z0-9]{20,})\b/g, 'key-***REDACTED***')
    .replace(/\b(AIza[a-zA-Z0-9_-]{35})\b/g, '***GOOGLE_KEY_REDACTED***')
    .replace(/Bearer\s+[a-zA-Z0-9._-]+/gi, 'Bearer ***REDACTED***');
}

// Default log directory
const LOG_DIR = validateLogDir(
  process.env.NEXIBOT_INFERENCE_LOG_DIR ||
    join(homedir(), '.config', 'nexibot', 'logs')
);

// Ensure log directory exists
if (!existsSync(LOG_DIR)) {
  try {
    mkdirSync(LOG_DIR, { recursive: true });
  } catch (e) {
    console.warn('[INFERENCE_LOG] Failed to create log directory:', e.message);
  }
}

const LOG_FILE = join(LOG_DIR, 'inference.jsonl');

/**
 * Log an inference event.
 * @param {object} entry
 * @param {string} entry.provider - Provider name (e.g., "anthropic", "openai", "google")
 * @param {string} entry.model - Model ID used
 * @param {number} entry.input_tokens - Input token count (estimated or from response)
 * @param {number} entry.output_tokens - Output token count
 * @param {number} entry.latency_ms - Total latency in milliseconds
 * @param {boolean} entry.streaming - Whether this was a streaming request
 * @param {string} [entry.error] - Error message if the call failed
 */
export function logInference(entry) {
  const logEntry = {
    timestamp: new Date().toISOString(),
    ...entry,
    error: entry.error ? sanitizeError(entry.error) : undefined,
  };

  try {
    appendFileSync(LOG_FILE, JSON.stringify(logEntry) + '\n');
  } catch (e) {
    // Don't crash on log write failure
    console.warn('[INFERENCE_LOG] Failed to write log:', e.message);
  }
}

/**
 * Express middleware that wraps request handlers to log inference metrics.
 * @param {string} provider - Provider name for logging
 * @returns {Function} Express middleware
 */
export function inferenceLogMiddleware(provider) {
  return (req, res, next) => {
    const startTime = Date.now();

    // Capture the original end/write to intercept response
    const originalJson = res.json.bind(res);
    const model = req.body?.model || 'unknown';

    res.json = function(data) {
      const latencyMs = Date.now() - startTime;

      logInference({
        provider,
        model,
        input_tokens: data?.usage?.input_tokens || 0,
        output_tokens: data?.usage?.output_tokens || 0,
        latency_ms: latencyMs,
        streaming: false,
        error: data?.error?.message || undefined,
      });

      return originalJson(data);
    };

    // For streaming endpoints, log on response finish
    res.on('finish', () => {
      if (req.path.includes('/stream')) {
        const latencyMs = Date.now() - startTime;
        logInference({
          provider,
          model,
          input_tokens: 0, // Not easily available in streaming
          output_tokens: 0,
          latency_ms: latencyMs,
          streaming: true,
        });
      }
    });

    next();
  };
}
