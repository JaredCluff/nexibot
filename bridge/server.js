/**
 * NexiBot Bridge Service
 *
 * Plugin-based bridge server that discovers and loads provider plugins at
 * startup. Each plugin exports a register(app, context) function that mounts
 * its own Express routes.
 *
 * Architecture:
 *   NexiBot (Rust) -> HTTP/SSE -> Bridge (Node.js) -> Provider APIs
 *
 * Plugin locations:
 *   - Built-in:  bridge/plugins/        (shipped with NexiBot)
 *   - External:  BRIDGE_PLUGINS_DIR     (user-installed, e.g. ~/.config/nexibot/bridge-plugins/)
 */

import express from 'express';
import cors from 'cors';
import { readdir, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { timingSafeEqual } from 'node:crypto';

import { normalizeMessages, validateAndRepairMessages } from './lib/normalize.js';
import { keyFingerprint } from './lib/utils.js';
import { PluginSDK } from './lib/plugin-sdk.js';
import searchRouter from './lib/search.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const app = express();
const PORT = process.env.BRIDGE_PORT || 18790;

// Restrict CORS to localhost origins only — the bridge should never be
// accessible from arbitrary web pages.
app.use(cors({
  origin: [
    'http://127.0.0.1',
    'http://localhost',
    'https://tauri.localhost',
    /^http:\/\/127\.0\.0\.1:\d+$/,
    /^http:\/\/localhost:\d+$/,
  ],
}));
app.use(express.json({ limit: '10mb' }));

// Shared-secret guard: when BRIDGE_SECRET is set (i.e. the Rust backend started
// this process), every non-health request must carry a matching
// X-Bridge-Secret header.  If the env var is absent (standalone / dev mode)
// the check is skipped for backward compatibility.
const BRIDGE_SECRET = process.env.BRIDGE_SECRET || null;
const _nodePath = require('path');
app.use((req, res, next) => {
  if (!BRIDGE_SECRET) return next();          // no secret configured — allow all
  // Normalize the path to prevent /health/.. traversal bypasses.
  const normalizedPath = _nodePath.posix.normalize(req.path || '/');
  if (normalizedPath === '/health') return next();  // health endpoint is always open

  const provided = req.headers['x-bridge-secret'];
  if (!provided) {
    return res.status(401).json({ error: 'Unauthorized: missing or invalid X-Bridge-Secret' });
  }
  // Use constant-time comparison to prevent timing side-channel attacks.
  const providedBuf = Buffer.from(provided);
  const secretBuf = Buffer.from(BRIDGE_SECRET);
  if (providedBuf.length !== secretBuf.length || !timingSafeEqual(providedBuf, secretBuf)) {
    return res.status(401).json({ error: 'Unauthorized: missing or invalid X-Bridge-Secret' });
  }
  next();
});

// Track loaded plugins
const loadedPlugins = [];
// Track plugin load failures for health reporting
const pluginLoadErrors = [];
// Track SDK v2 plugin instances for health reporting
const pluginSDKs = [];

/**
 * Load plugins from a directory.
 * Each subdirectory with a plugin.json is treated as a plugin.
 */
async function loadPluginsFromDir(pluginsDir, label) {
  if (!existsSync(pluginsDir)) {
    console.log(`[Bridge] Plugin directory not found (${label}): ${pluginsDir}`);
    return;
  }

  let entries;
  try {
    entries = await readdir(pluginsDir, { withFileTypes: true });
  } catch (err) {
    console.error(`[Bridge] Failed to read plugin directory (${label}):`, err.message);
    return;
  }

  for (const entry of entries) {
    if (!entry.isDirectory()) continue;

    const pluginDir = path.join(pluginsDir, entry.name);
    const manifestPath = path.join(pluginDir, 'plugin.json');

    if (!existsSync(manifestPath)) {
      continue;
    }

    try {
      const manifestRaw = await readFile(manifestPath, 'utf-8');
      const manifest = JSON.parse(manifestRaw);

      // Validate required manifest fields
      if (!manifest.name || typeof manifest.name !== 'string') {
        console.warn(`[Bridge] Skipping plugin in '${entry.name}': missing or invalid 'name' in plugin.json`);
        continue;
      }

      if (!manifest.version || typeof manifest.version !== 'string') {
        console.warn(`[Bridge] Skipping plugin '${manifest.name}': missing or invalid 'version' in plugin.json`);
        continue;
      }

      // Validate bridge API version (v1 and v2 supported)
      const apiVersion = manifest.bridge_api_version || '1';
      if (apiVersion !== '1' && apiVersion !== '2') {
        console.warn(`[Bridge] Skipping plugin '${manifest.name}': unsupported bridge_api_version '${manifest.bridge_api_version}'`);
        continue;
      }

      // Validate and resolve the plugin entry path — must stay within the plugin directory
      const entryFile = manifest.entry || 'index.js';
      const entryPath = path.resolve(pluginDir, entryFile);
      if (!entryPath.startsWith(pluginDir + path.sep) && entryPath !== pluginDir) {
        console.warn(`[Bridge] Skipping plugin '${manifest.name}': entry path '${entryFile}' escapes plugin directory`);
        continue;
      }

      const plugin = await import(entryPath);

      if (typeof plugin.register !== 'function') {
        console.warn(`[Bridge] Skipping plugin '${manifest.name}': no register() export`);
        continue;
      }

      // Build context object passed to plugins
      const context = {
        utils: {
          normalizeMessages,
          validateAndRepairMessages,
          keyFingerprint,
        },
        logger: console,
      };

      // Register plugin routes (v1 or v2)
      let sdk = null;
      if (apiVersion === '2') {
        sdk = new PluginSDK(app, context, manifest.name);
        plugin.register(sdk);
        pluginSDKs.push(sdk);
      } else {
        plugin.register(app, context);
      }

      const info = {
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        source: label,
        bridge_api_version: apiVersion,
        health: typeof plugin.health === 'function' ? plugin.health() : null,
        ...(sdk ? {
          sdk_providers: sdk.providers.map(p => p.name),
          sdk_tools: sdk.tools.map(t => t.name),
          sdk_channels: sdk.channels.map(c => c.name),
        } : {}),
      };

      loadedPlugins.push(info);
      console.log(`[Bridge] Loaded plugin: ${manifest.name} v${manifest.version} (${label})`);

    } catch (err) {
      console.error(`[Bridge] Failed to load plugin '${entry.name}' (${label}):`, err.message);
      pluginLoadErrors.push({ plugin: entry.name, source: label, error: err.message });
    }
  }
}

/**
 * Health check endpoint — returns loaded plugin list
 */
app.get('/health', (req, res) => {
  const hasDegraded = pluginLoadErrors.length > 0;
  res.json({
    status: hasDegraded ? 'degraded' : 'healthy',
    service: 'nexibot-bridge',
    version: '1.0.0',
    timestamp: new Date().toISOString(),
    plugins: loadedPlugins.map(p => ({
      name: p.name,
      version: p.version,
      source: p.source,
      bridge_api_version: p.bridge_api_version,
      health: p.health,
      ...(p.sdk_providers ? { sdk_providers: p.sdk_providers } : {}),
      ...(p.sdk_tools ? { sdk_tools: p.sdk_tools } : {}),
      ...(p.sdk_channels ? { sdk_channels: p.sdk_channels } : {}),
    })),
    plugin_load_errors: pluginLoadErrors,
  });
});

// Mount search router (core service, not a plugin)
app.use(searchRouter);

/**
 * Shutdown handler
 */
process.on('SIGINT', () => {
  console.log('[Bridge] Shutting down...');
  process.exit(0);
});

process.on('SIGTERM', () => {
  console.log('[Bridge] Shutting down...');
  process.exit(0);
});

process.on('uncaughtException', (err) => {
  console.error('[BRIDGE] Uncaught exception:', err);
  // Don't exit — let Express's default error handler return 500 to callers
});

process.on('unhandledRejection', (reason) => {
  console.error('[BRIDGE] Unhandled promise rejection:', reason);
});

/**
 * Start server after loading plugins
 */
async function main() {
  // 1. Load built-in plugins
  const builtinPluginsDir = path.join(__dirname, 'plugins');
  await loadPluginsFromDir(builtinPluginsDir, 'built-in');

  // 2. Load external plugins (user-installed)
  const externalPluginsDir = process.env.BRIDGE_PLUGINS_DIR;
  if (externalPluginsDir) {
    await loadPluginsFromDir(externalPluginsDir, 'external');
  }

  // Start listening
  app.listen(PORT, '127.0.0.1', () => {
    console.log('');
    console.log('  NexiBot Bridge Service');
    console.log('  ----------------------');
    console.log(`  Listening on http://127.0.0.1:${PORT}`);
    console.log(`  Loaded ${loadedPlugins.length} plugin(s):`);
    for (const p of loadedPlugins) {
      console.log(`    - ${p.name} v${p.version} (${p.source})`);
    }
    console.log('');
    console.log('  Core endpoints:');
    console.log(`    GET  http://127.0.0.1:${PORT}/health`);
    console.log(`    POST http://127.0.0.1:${PORT}/api/search`);
    console.log('');
    console.log('  Press Ctrl+C to stop');
    console.log('');
  });
}

main().catch(err => {
  console.error('[Bridge] Fatal error during startup:', err);
  process.exit(1);
});
