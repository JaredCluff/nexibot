/**
 * Claude Code tool name casing conversion.
 *
 * When using OAuth, tool names must match Claude Code's canonical casing
 * (e.g. "read" -> "Read", "write" -> "Write").
 */

const claudeCodeTools = [
  'Read', 'Write', 'Edit', 'Bash', 'Grep', 'Glob',
  'AskUserQuestion', 'EnterPlanMode', 'ExitPlanMode',
  'KillShell', 'NotebookEdit', 'Skill', 'Task',
  'TaskOutput', 'TodoWrite', 'WebFetch', 'WebSearch',
];

export const ccToolLookup = new Map(
  claudeCodeTools.map(t => [t.toLowerCase(), t])
);

/**
 * Convert a tool name to Claude Code canonical casing.
 */
export function toClaudeCodeName(name) {
  return ccToolLookup.get(name.toLowerCase()) || name;
}

/**
 * Convert all tools in an array to Claude Code casing for OAuth requests.
 */
export function convertToolsForOAuth(tools, isOAuth) {
  if (!tools || !Array.isArray(tools)) return tools;
  if (!isOAuth) return tools;

  return tools.map(tool => ({
    ...tool,
    name: toClaudeCodeName(tool.name),
  }));
}
