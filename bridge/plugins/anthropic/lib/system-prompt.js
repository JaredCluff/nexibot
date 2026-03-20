/**
 * System prompt building with Claude Code identity injection for OAuth tokens.
 */

/**
 * Build system prompt with Claude Code identity for OAuth tokens.
 *
 * When using OAuth, prepends the Claude Code identity block so that
 * Anthropic's API recognizes the request as coming from Claude Code.
 */
export function buildSystemPrompt(isOAuth, userSystemPrompt) {
  if (isOAuth) {
    const ccIdentity = {
      type: 'text',
      text: 'You are Claude Code, Anthropic\'s official CLI for Claude.',
    };

    if (userSystemPrompt) {
      if (typeof userSystemPrompt === 'string') {
        return [
          ccIdentity,
          { type: 'text', text: userSystemPrompt }
        ];
      } else if (Array.isArray(userSystemPrompt)) {
        return [ccIdentity, ...userSystemPrompt];
      }
    }

    return [ccIdentity];
  }

  return userSystemPrompt;
}
