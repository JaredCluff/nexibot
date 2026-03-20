/**
 * Shared utility functions for the bridge service.
 */

import { createHash } from 'node:crypto';

/**
 * Return the first 8 hex chars of SHA-256(key) for log identification
 * without exposing any key material.
 */
export function keyFingerprint(key) {
  return createHash('sha256').update(key).digest('hex').substring(0, 8);
}
