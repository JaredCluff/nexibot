/**
 * Message normalization utilities for the Anthropic API.
 *
 * Handles converting stringified JSON content back to arrays and
 * repairing orphaned tool_use/tool_result pairings.
 */

/**
 * Normalize message content for the Anthropic API.
 *
 * The Rust client stores assistant content blocks (tool_use) and user content
 * blocks (tool_result) as serialized JSON strings in Message.content.
 * The API expects these as actual JSON arrays, not strings.
 * This function detects stringified arrays and parses them back.
 */
export function normalizeMessages(messages) {
  return messages.map(msg => {
    if (typeof msg.content === 'string' && msg.content.trimStart().startsWith('[')) {
      try {
        const parsed = JSON.parse(msg.content);
        if (Array.isArray(parsed) && parsed.length > 0 && typeof parsed[0] === 'object' && parsed[0].type) {
          return { ...msg, content: parsed };
        }
      } catch {
        // Not valid JSON — leave as string
      }
    }
    return msg;
  });
}

/**
 * Validate and repair tool_use/tool_result pairing in normalized messages.
 *
 * The Anthropic API requires that every tool_result block in a user message
 * references a tool_use block in the IMMEDIATELY PRECEDING assistant message.
 * History trimming in the Rust client can occasionally produce mismatched pairs;
 * this function strips orphaned tool_result blocks before the API call so the
 * request is never rejected with "unexpected tool_use_id".
 *
 * Must be called AFTER normalizeMessages so content is already parsed to arrays.
 */
export function validateAndRepairMessages(messages) {
  const repaired = [...messages];
  let repairs = 0;

  for (let i = 1; i < repaired.length; i++) {
    const msg = repaired[i];
    if (msg.role !== 'user') continue;
    const content = msg.content;
    if (!Array.isArray(content)) continue;

    const toolResults = content.filter(b => b.type === 'tool_result');
    if (toolResults.length === 0) continue;

    const prev = repaired[i - 1];
    const prevContent = (prev && Array.isArray(prev.content)) ? prev.content : [];
    const validIds = new Set(
      prevContent.filter(b => b.type === 'tool_use' && b.id).map(b => b.id)
    );

    if (prev && prev.role !== 'assistant') {
      // No preceding assistant message at all — every tool_result here is orphaned
      console.warn(`[Bridge] Repair: user[${i}] has tool_results but no preceding assistant — removing all`);
      const cleaned = content.filter(b => b.type !== 'tool_result');
      repairs += toolResults.length;
      if (cleaned.length === 0) {
        repaired.splice(i, 1);
        i--;
      } else {
        repaired[i] = { ...msg, content: cleaned };
      }
      continue;
    }

    const cleanedContent = content.filter(b => {
      if (b.type !== 'tool_result') return true;
      if (validIds.has(b.tool_use_id)) return true;
      console.warn(`[Bridge] Repair: removing orphaned tool_result ${b.tool_use_id} (not in preceding assistant tool_uses)`);
      repairs++;
      return false;
    });

    if (cleanedContent.length !== content.length) {
      if (cleanedContent.length === 0) {
        repaired.splice(i, 1);
        i--;
      } else {
        repaired[i] = { ...msg, content: cleanedContent };
      }
    }
  }

  if (repairs > 0) {
    console.warn(`[Bridge] Repaired ${repairs} orphaned tool_result block(s) before API call`);
  }
  return repaired;
}
