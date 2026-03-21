/**
 * NexiBot Bridge Plugin SDK v2
 *
 * Provides registration helpers for bridge plugins. Plugins using v2
 * get an instance of PluginSDK in their `register` callback instead
 * of raw `(app, context)`.
 *
 * Backward compatible: v1 plugins (`register(app, context)`) continue
 * working unchanged.
 */

// Global registry of registered tool names to prevent collision/hijacking
const _registeredToolNames = new Set();
// Global registry of registered route paths to prevent shadowing
const _registeredRoutes = new Set();

class PluginSDK {
  /**
   * @param {import('express').Application} app - Express application
   * @param {object} context - Bridge context (logger, config, etc.)
   * @param {string} pluginId - Unique plugin identifier
   */
  constructor(app, context, pluginId) {
    this._app = app;
    this._context = context;
    this._pluginId = pluginId;
    this._providers = [];
    this._tools = [];
    this._channels = [];
    this._speechProviders = [];
    this._hooks = {};
  }

  /**
   * Register an LLM provider.
   * @param {object} opts
   * @param {string} opts.name - Provider name (e.g., "google", "deepseek")
   * @param {string[]} opts.models - Supported model IDs
   * @param {string} opts.streamEndpoint - Route path for streaming (e.g., "/api/google/messages/stream")
   * @param {string} opts.messageEndpoint - Route path for non-streaming
   * @param {Function} opts.streamHandler - Express handler for streaming
   * @param {Function} opts.messageHandler - Express handler for non-streaming
   * @param {Function} [opts.modelsHandler] - Express handler for listing models
   */
  registerProvider({ name, models, streamEndpoint, messageEndpoint, streamHandler, messageHandler, modelsHandler }) {
    if (!name || !streamEndpoint || !messageEndpoint) {
      throw new Error(`registerProvider: name, streamEndpoint, and messageEndpoint are required`);
    }

    // Prevent route shadowing across plugins
    for (const route of [streamEndpoint, messageEndpoint]) {
      if (_registeredRoutes.has(route)) {
        this._context.logger?.warn?.(
          `[SDK] Route '${route}' already registered — skipping provider '${name}' (plugin: ${this._pluginId})`
        );
        return;
      }
    }

    this._providers.push({ name, models: models || [] });

    // Mount routes
    _registeredRoutes.add(streamEndpoint);
    _registeredRoutes.add(messageEndpoint);
    this._app.post(streamEndpoint, streamHandler);
    this._app.post(messageEndpoint, messageHandler);

    if (modelsHandler) {
      const modelsPath = streamEndpoint.replace(/\/messages\/stream$/, '/models');
      _registeredRoutes.add(modelsPath);
      this._app.get(modelsPath, modelsHandler);
    }

    this._context.logger?.info?.(`[SDK] Registered provider: ${name} (${(models || []).length} models)`);
  }

  /**
   * Register a tool that can be invoked by the LLM.
   * @param {object} opts
   * @param {string} opts.name - Tool name
   * @param {string} opts.description - Tool description
   * @param {object} opts.inputSchema - JSON Schema for tool input
   * @param {Function} opts.handler - async (input) => result
   */
  registerTool({ name, description, inputSchema, handler }) {
    if (!name || !handler) {
      throw new Error(`registerTool: name and handler are required`);
    }

    // Prevent tool name collision/hijacking across plugins
    if (_registeredToolNames.has(name)) {
      this._context.logger?.warn?.(
        `[SDK] Tool '${name}' already registered by another plugin — skipping (plugin: ${this._pluginId})`
      );
      return;
    }
    _registeredToolNames.add(name);

    this._tools.push({ name, description, inputSchema, handler });

    // Mount tool execution endpoint
    const routePath = `/api/tools/${name}`;
    _registeredRoutes.add(routePath);
    this._app.post(routePath, async (req, res) => {
      try {
        const result = await handler(req.body);
        res.json({ result });
      } catch (err) {
        res.status(500).json({ error: err.message });
      }
    });

    this._context.logger?.info?.(`[SDK] Registered tool: ${name}`);
  }

  /**
   * Register a messaging channel.
   * @param {object} opts
   * @param {string} opts.name - Channel name
   * @param {Function} opts.inbound - Express handler for inbound messages
   * @param {Function} opts.outbound - async (message) => void
   */
  registerChannel({ name, inbound, outbound }) {
    if (!name) {
      throw new Error(`registerChannel: name is required`);
    }

    this._channels.push({ name, outbound });

    if (inbound) {
      this._app.post(`/api/channels/${name}/inbound`, inbound);
    }

    this._context.logger?.info?.(`[SDK] Registered channel: ${name}`);
  }

  /**
   * Register a speech provider (STT and/or TTS).
   * @param {object} opts
   * @param {string} opts.name - Provider name
   * @param {Function} [opts.stt] - async (audioBuffer, options) => { text, confidence }
   * @param {Function} [opts.tts] - async (text, options) => audioBuffer
   */
  registerSpeechProvider({ name, stt, tts }) {
    if (!name) {
      throw new Error(`registerSpeechProvider: name is required`);
    }

    this._speechProviders.push({ name, stt, tts });

    if (stt) {
      this._app.post(`/api/speech/${name}/stt`, async (req, res) => {
        try {
          const result = await stt(req.body.audio, req.body.options);
          res.json(result);
        } catch (err) {
          res.status(500).json({ error: err.message });
        }
      });
    }

    if (tts) {
      this._app.post(`/api/speech/${name}/tts`, async (req, res) => {
        try {
          const audioBuffer = await tts(req.body.text, req.body.options);
          res.set('Content-Type', 'audio/wav');
          res.send(audioBuffer);
        } catch (err) {
          res.status(500).json({ error: err.message });
        }
      });
    }

    this._context.logger?.info?.(`[SDK] Registered speech provider: ${name}`);
  }

  /**
   * Register a hook handler.
   * @param {string} point - Hook point (e.g., "before_message", "after_message", "before_tool", "after_tool")
   * @param {Function} handler - async (data) => data | null
   */
  registerHook(point, handler) {
    if (!this._hooks[point]) {
      this._hooks[point] = [];
    }
    this._hooks[point].push(handler);
    this._context.logger?.info?.(`[SDK] Registered hook: ${point} (${this._pluginId})`);
  }

  /**
   * Get registered providers.
   */
  get providers() {
    return this._providers;
  }

  /**
   * Get registered tools.
   */
  get tools() {
    return this._tools;
  }

  /**
   * Get registered channels.
   */
  get channels() {
    return this._channels;
  }

  /**
   * Get registered hooks.
   */
  get hooks() {
    return this._hooks;
  }

  /**
   * Get the Express app instance (for advanced use cases).
   */
  get app() {
    return this._app;
  }

  /**
   * Get the bridge context.
   */
  get context() {
    return this._context;
  }
}

export { PluginSDK };
