export interface TagSuggestion {
  value: string;
  description: string;
  badge?: 'dangerous' | 'elevated' | 'safe' | 'pattern';
}

export const BUILTIN_TOOL_SUGGESTIONS: TagSuggestion[] = [
  { value: 'nexibot_execute', description: 'Run shell commands on the host', badge: 'dangerous' },
  { value: 'nexibot_fetch', description: 'Fetch URLs and web content', badge: 'elevated' },
  { value: 'nexibot_filesystem_read', description: 'Read files from disk', badge: 'elevated' },
  { value: 'nexibot_filesystem_write', description: 'Write/create files on disk', badge: 'dangerous' },
  { value: 'nexibot_memory_store', description: 'Store data in long-term memory', badge: 'safe' },
  { value: 'nexibot_memory_search', description: 'Search long-term memory', badge: 'safe' },
  { value: 'nexibot_search', description: 'Search indexed files and knowledge', badge: 'safe' },
  { value: 'nexibot_computer_use', description: 'Control mouse, keyboard, screenshots', badge: 'dangerous' },
];

export const PATTERN_SUGGESTIONS: TagSuggestion[] = [
  { value: 'execute_*', description: 'Any tool starting with execute_', badge: 'pattern' },
  { value: 'delete_*', description: 'Any tool starting with delete_', badge: 'pattern' },
  { value: 'rm_*', description: 'Any tool starting with rm_', badge: 'pattern' },
  { value: 'drop_*', description: 'Any tool starting with drop_', badge: 'pattern' },
  { value: 'destroy_*', description: 'Any tool starting with destroy_', badge: 'pattern' },
  { value: 'kill_*', description: 'Any tool starting with kill_', badge: 'pattern' },
  { value: '*_admin', description: 'Any tool ending with _admin', badge: 'pattern' },
  { value: '*_sudo', description: 'Any tool ending with _sudo', badge: 'pattern' },
];

/** Return platform-appropriate safe command suggestions. */
export function getSafeCommandSuggestions(os?: string): TagSuggestion[] {
  const platform = os || detectPlatform();
  const common: TagSuggestion[] = [
    { value: 'echo', description: 'Print text to stdout', badge: 'safe' },
    { value: 'python', description: 'Run Python scripts', badge: 'elevated' },
    { value: 'python3', description: 'Run Python 3 scripts', badge: 'elevated' },
    { value: 'node', description: 'Run Node.js scripts', badge: 'elevated' },
    { value: 'git', description: 'Git version control', badge: 'safe' },
    { value: 'curl', description: 'Transfer data from URLs', badge: 'elevated' },
  ];
  if (platform === 'windows') {
    return [
      ...common,
      { value: 'dir', description: 'List directory contents', badge: 'safe' },
      { value: 'cd', description: 'Print or change working directory', badge: 'safe' },
      { value: 'type', description: 'Display file contents', badge: 'safe' },
      { value: 'findstr', description: 'Search text patterns in files', badge: 'safe' },
      { value: 'where', description: 'Locate a command', badge: 'safe' },
      { value: 'whoami', description: 'Print current username', badge: 'safe' },
      { value: 'set', description: 'Display environment variables', badge: 'safe' },
      { value: 'Get-ChildItem', description: 'List items (PowerShell)', badge: 'safe' },
      { value: 'Get-Content', description: 'Read file contents (PowerShell)', badge: 'safe' },
    ];
  }
  return [
    ...common,
    { value: 'ls', description: 'List directory contents', badge: 'safe' },
    { value: 'pwd', description: 'Print working directory', badge: 'safe' },
    { value: 'cat', description: 'Display file contents', badge: 'safe' },
    { value: 'head', description: 'Display first lines of a file', badge: 'safe' },
    { value: 'tail', description: 'Display last lines of a file', badge: 'safe' },
    { value: 'grep', description: 'Search text patterns in files', badge: 'safe' },
    { value: 'find', description: 'Search for files', badge: 'safe' },
    { value: 'wc', description: 'Count lines/words/characters', badge: 'safe' },
    { value: 'date', description: 'Display current date/time', badge: 'safe' },
    { value: 'whoami', description: 'Print current username', badge: 'safe' },
    { value: 'which', description: 'Locate a command', badge: 'safe' },
    { value: 'env', description: 'Display environment variables', badge: 'safe' },
  ];
}

/** Return platform-appropriate dangerous command suggestions. */
export function getDangerousCommandSuggestions(os?: string): TagSuggestion[] {
  const platform = os || detectPlatform();
  const common: TagSuggestion[] = [
    { value: 'shutdown', description: 'Shut down the system', badge: 'dangerous' },
    { value: 'reboot', description: 'Reboot the system', badge: 'dangerous' },
  ];
  if (platform === 'windows') {
    return [
      ...common,
      { value: 'del', description: 'Delete files', badge: 'dangerous' },
      { value: 'rmdir', description: 'Remove directories', badge: 'dangerous' },
      { value: 'format', description: 'Format a disk volume', badge: 'dangerous' },
      { value: 'taskkill', description: 'Terminate processes', badge: 'dangerous' },
      { value: 'icacls', description: 'Change file permissions', badge: 'elevated' },
      { value: 'net', description: 'Network configuration', badge: 'dangerous' },
      { value: 'reg', description: 'Modify Windows registry', badge: 'dangerous' },
      { value: 'sc', description: 'Control Windows services', badge: 'dangerous' },
      { value: 'runas', description: 'Execute as another user', badge: 'dangerous' },
      { value: 'Remove-Item', description: 'Remove items (PowerShell)', badge: 'dangerous' },
      { value: 'Stop-Process', description: 'Terminate processes (PowerShell)', badge: 'dangerous' },
    ];
  }
  return [
    ...common,
    { value: 'rm', description: 'Remove files and directories', badge: 'dangerous' },
    { value: 'dd', description: 'Low-level disk copy (can destroy data)', badge: 'dangerous' },
    { value: 'mkfs', description: 'Format a filesystem', badge: 'dangerous' },
    { value: 'sudo', description: 'Execute as superuser', badge: 'dangerous' },
    { value: 'kill', description: 'Terminate processes', badge: 'dangerous' },
    { value: 'killall', description: 'Terminate processes by name', badge: 'dangerous' },
    { value: 'chmod', description: 'Change file permissions', badge: 'elevated' },
    { value: 'chown', description: 'Change file ownership', badge: 'elevated' },
    { value: 'mount', description: 'Mount filesystems', badge: 'dangerous' },
    { value: 'umount', description: 'Unmount filesystems', badge: 'dangerous' },
    { value: 'fdisk', description: 'Partition disk', badge: 'dangerous' },
    { value: 'iptables', description: 'Modify firewall rules', badge: 'dangerous' },
    ...(platform === 'linux'
      ? [{ value: 'systemctl', description: 'Control system services', badge: 'dangerous' as const }]
      : [{ value: 'launchctl', description: 'Control macOS services', badge: 'dangerous' as const }]),
  ];
}

// Platform-aware exports using runtime detection
export const SAFE_COMMAND_SUGGESTIONS: TagSuggestion[] = getSafeCommandSuggestions();
export const DANGEROUS_COMMAND_SUGGESTIONS: TagSuggestion[] = getDangerousCommandSuggestions();

/** Return platform-appropriate sensitive path suggestions. */
export function getSensitivePathSuggestions(os?: string): TagSuggestion[] {
  const platform = os || detectPlatform();
  const common: TagSuggestion[] = [
    { value: '~/.ssh', description: 'SSH keys and config', badge: 'dangerous' },
    { value: '~/.aws', description: 'AWS credentials and config', badge: 'dangerous' },
    { value: '~/.gnupg', description: 'GPG keys and config', badge: 'dangerous' },
  ];

  if (platform === 'windows') {
    return [
      ...common,
      { value: 'C:\\Windows', description: 'Windows system directory', badge: 'dangerous' },
      { value: 'C:\\Windows\\System32', description: 'System binaries', badge: 'dangerous' },
      { value: '%APPDATA%', description: 'Application data (roaming)', badge: 'elevated' },
      { value: '%LOCALAPPDATA%', description: 'Application data (local)', badge: 'elevated' },
      { value: '%USERPROFILE%\\.config', description: 'User application config', badge: 'elevated' },
    ];
  }
  if (platform === 'linux') {
    return [
      ...common,
      { value: '/etc', description: 'System configuration files', badge: 'dangerous' },
      { value: '/proc', description: 'Process information', badge: 'dangerous' },
      { value: '/sys', description: 'Kernel parameters', badge: 'dangerous' },
      { value: '/dev', description: 'Device files', badge: 'dangerous' },
      { value: '/boot', description: 'Boot loader files', badge: 'dangerous' },
      { value: '/sbin', description: 'System binaries', badge: 'dangerous' },
      { value: '~/.config', description: 'User application config', badge: 'elevated' },
      { value: '/var/log', description: 'System log files', badge: 'elevated' },
    ];
  }
  // macOS
  return [
    ...common,
    { value: '/etc', description: 'System configuration files', badge: 'dangerous' },
    { value: '/dev', description: 'Device files', badge: 'dangerous' },
    { value: '/sbin', description: 'System binaries', badge: 'dangerous' },
    { value: '~/.config', description: 'User application config', badge: 'elevated' },
    { value: '/System', description: 'macOS system files', badge: 'dangerous' },
    { value: '/Library', description: 'macOS shared libraries', badge: 'elevated' },
    { value: '/var/log', description: 'System log files', badge: 'elevated' },
  ];
}

/** Return platform-appropriate safe path suggestions. */
export function getSafePathSuggestions(os?: string): TagSuggestion[] {
  const platform = os || detectPlatform();
  if (platform === 'windows') {
    return [
      { value: '%TEMP%', description: 'Temporary files', badge: 'safe' },
    ];
  }
  return [
    { value: '/tmp', description: 'Temporary files', badge: 'safe' },
    { value: '/var/tmp', description: 'Persistent temporary files', badge: 'safe' },
  ];
}

/** Detect the user's platform from navigator.userAgent (best-effort). */
function detectPlatform(): 'windows' | 'macos' | 'linux' {
  if (typeof navigator !== 'undefined') {
    const ua = navigator.userAgent.toLowerCase();
    if (ua.includes('win')) return 'windows';
    if (ua.includes('mac')) return 'macos';
  }
  return 'linux';
}

// Platform-aware exports using runtime detection
export const SENSITIVE_PATH_SUGGESTIONS: TagSuggestion[] = getSensitivePathSuggestions();
export const SAFE_PATH_SUGGESTIONS: TagSuggestion[] = getSafePathSuggestions();

export const ALLOWED_DOMAIN_SUGGESTIONS: TagSuggestion[] = [
  { value: 'api.github.com', description: 'GitHub REST API', badge: 'safe' },
  { value: 'api.openai.com', description: 'OpenAI API', badge: 'safe' },
  { value: 'api.anthropic.com', description: 'Anthropic API', badge: 'safe' },
  { value: 'raw.githubusercontent.com', description: 'GitHub raw file hosting', badge: 'safe' },
  { value: 'registry.npmjs.org', description: 'npm package registry', badge: 'safe' },
  { value: 'pypi.org', description: 'Python package index', badge: 'safe' },
  { value: 'crates.io', description: 'Rust package registry', badge: 'safe' },
  { value: 'api.duckduckgo.com', description: 'DuckDuckGo search API', badge: 'safe' },
];

export const BLOCKED_DOMAIN_SUGGESTIONS: TagSuggestion[] = [
  { value: 'localhost', description: 'Local loopback (hostname)', badge: 'dangerous' },
  { value: '127.0.0.1', description: 'IPv4 loopback address', badge: 'dangerous' },
  { value: '::1', description: 'IPv6 loopback address', badge: 'dangerous' },
  { value: '169.254.169.254', description: 'Cloud metadata endpoint (SSRF)', badge: 'dangerous' },
  { value: '0.0.0.0', description: 'All interfaces / wildcard', badge: 'dangerous' },
  { value: 'metadata.google.internal', description: 'GCP metadata service', badge: 'dangerous' },
];
