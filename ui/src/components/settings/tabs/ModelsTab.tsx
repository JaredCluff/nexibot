import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { notifyError, notifyInfo } from '../../../shared/notify';
import { InfoTip } from '../shared/InfoTip';
import { CollapsibleSection } from '../shared/CollapsibleSection';

const getDefaultMaxTokens = (modelId: string): number => {
  if (modelId.startsWith('claude-opus') || modelId.startsWith('claude-sonnet-4')) return 16384;
  if (modelId.startsWith('claude-')) return 8192;
  if (modelId.startsWith('gpt-4o')) return 16384;
  if (modelId.startsWith('gpt-4')) return 8192;
  if (modelId.startsWith('gpt-3.5')) return 4096;
  if (modelId.startsWith('gemini-')) return 8192;
  if (modelId.startsWith('deepseek-')) return 8192;
  if (modelId.includes('llama')) return 4096;
  return 4096;
};

export function ModelsTab() {
  const { config, setConfig, availableModels, modelsLoading, loadModels } = useSettings();
  if (!config) return null;

  const providers = [...new Set(availableModels.map(m => m.provider))];

  return (
    <div className="tab-content">
      <div className="settings-group">
        <h3>Primary Model<InfoTip text="The main AI model used for conversations. Different models have different capabilities, speed, and cost trade-offs." /></h3>
        <p className="group-description">Select a model for conversations. Models are fetched from your configured providers.</p>
        {modelsLoading ? (
          <div className="loading-models">Loading available models...</div>
        ) : availableModels.length === 0 ? (
          <div className="no-models">
            <p>No models available. Configure an API key below or check that the bridge service is running.</p>
            <button className="secondary-button" onClick={loadModels} disabled={modelsLoading}>{modelsLoading ? 'Loading…' : 'Retry'}</button>
          </div>
        ) : (
          <div className="model-list">
            {providers.map(provider => (
              <div key={provider} className="model-provider-group">
                <h4 className="provider-label">{provider}</h4>
                {availableModels.filter(m => m.provider === provider).sort((a, b) => (b.size_score || 0) - (a.size_score || 0)).map((model) => (
                  <label
                    key={model.id}
                    className={`model-card ${config.claude.model === model.id ? 'selected' : ''}`}
                  >
                    <input
                      type="radio"
                      name="primary-model"
                      value={model.id}
                      checked={config.claude.model === model.id}
                      onChange={() => setConfig({ ...config, claude: { ...config.claude, model: model.id, max_tokens: getDefaultMaxTokens(model.id) } })}
                    />
                    <div className="model-info">
                      <span className="model-name">{model.display_name}</span>
                      {model.id !== model.display_name && <span className="model-id">{model.id}</span>}
                    </div>
                  </label>
                ))}
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="settings-group">
        <h3>Fallback Model<InfoTip text="Automatically used when the primary model is unavailable or rate-limited. Leave set to 'None' if you don't need automatic failover." /></h3>
        <p className="group-description">Used when the primary model is unavailable or rate-limited.</p>
        <select
          value={config.claude.fallback_model || ''}
          onChange={(e) => setConfig({
            ...config,
            claude: { ...config.claude, fallback_model: e.target.value || undefined },
          })}
        >
          <option value="">None (no fallback)</option>
          {availableModels.filter(m => m.id !== config.claude.model).map((model) => (
            <option key={model.id} value={model.id}>{model.display_name}</option>
          ))}
        </select>
      </div>

      <CollapsibleSection title="Smart Routing" defaultOpen={true}>
        <p className="group-description">
          Route queries to the right model based on complexity — fast models for simple questions, powerful models for deep reasoning.
          Voice queries use the same complexity tiers but prefer faster models for simple turns.
        </p>
        <div className="settings-row" style={{ marginBottom: '8px' }}>
          <label className="field" style={{ flexDirection: 'row', alignItems: 'center', gap: '8px' }}>
            <input
              type="checkbox"
              checked={config.routing?.enabled ?? true}
              onChange={(e) => setConfig({ ...config, routing: { ...config.routing, enabled: e.target.checked, voice_latency_bias: config.routing?.voice_latency_bias ?? true, purposes: config.routing?.purposes ?? {} } })}
            />
            <span>Enable smart routing<InfoTip text="When enabled, queries are automatically routed to the most appropriate model based on complexity. Session overrides always take priority." /></span>
          </label>
          <label className="field" style={{ flexDirection: 'row', alignItems: 'center', gap: '8px' }}>
            <input
              type="checkbox"
              checked={config.routing?.voice_latency_bias ?? true}
              onChange={(e) => setConfig({ ...config, routing: { ...config.routing, enabled: config.routing?.enabled ?? true, voice_latency_bias: e.target.checked, purposes: config.routing?.purposes ?? {} } })}
            />
            <span>Voice latency bias<InfoTip text="For voice queries with no strong complexity signal, prefer faster models (voice_default) over the global default." /></span>
          </label>
        </div>

        {[
          { key: 'quick_chat',    label: 'Quick chat',          tip: 'Short, trivial conversational turns (under ~60 words, no code, no reasoning). Voice simple questions also use this tier.' },
          { key: 'code_simple',   label: 'Simple code',         tip: 'Short code tasks without architecture or debugging complexity.' },
          { key: 'code_complex',  label: 'Complex code',        tip: 'Architecture design, debugging, refactoring, or longer code prompts.' },
          { key: 'reasoning',     label: 'Reasoning / analysis',tip: 'Queries with analyze, compare, evaluate, critique, step-by-step, or math content.' },
          { key: 'long_context',  label: 'Long context',        tip: 'Prompts over ~800 words (document analysis, summarization).' },
          { key: 'agentic',       label: 'Agentic tasks',       tip: 'Multi-step planning, orchestration, design a system, build a pipeline.' },
          { key: 'voice_default', label: 'Voice default',       tip: 'Voice queries that match no complexity tier (simple factual, chitchat). Should be a fast model.' },
        ].map(({ key, label, tip }) => (
          <div key={key} className="settings-row">
            <label className="field">
              <span>{label}<InfoTip text={tip} /></span>
              <select
                value={(config.routing?.purposes as Record<string, string | undefined>)?.[key] ?? ''}
                onChange={(e) => setConfig({
                  ...config,
                  routing: {
                    enabled: config.routing?.enabled ?? true,
                    voice_latency_bias: config.routing?.voice_latency_bias ?? true,
                    purposes: { ...config.routing?.purposes, [key]: e.target.value || undefined },
                  },
                })}
              >
                <option value="">Default (use primary model)</option>
                {availableModels.map((m) => (
                  <option key={m.id} value={m.id}>{m.display_name}</option>
                ))}
              </select>
            </label>
          </div>
        ))}
      </CollapsibleSection>

      <div className="settings-row">
        <div className="settings-group compact">
          <h3>Max Tokens<InfoTip text="Maximum number of tokens in each response. Higher values allow longer responses but cost more. 1 token is roughly 4 characters." /></h3>
          <input
            type="number"
            min="256"
            max="32768"
            step="256"
            value={config.claude.max_tokens}
            onChange={(e) => setConfig({
              ...config,
              claude: { ...config.claude, max_tokens: parseInt(e.target.value) || 4096 },
            })}
          />
        </div>
      </div>

      <div className="settings-group">
        <h3>System Prompt<InfoTip text="Instructions given to the model before every conversation. Use this to set the assistant's personality, behavior, or domain expertise." /></h3>
        <textarea
          value={config.claude.system_prompt}
          onChange={(e) => setConfig({
            ...config,
            claude: { ...config.claude, system_prompt: e.target.value },
          })}
          rows={3}
        />
      </div>

      <div className="settings-group">
        <h3>OpenAI API Key<InfoTip text="Required to access GPT-4o and other OpenAI models. Without a key, only non-OpenAI models will be available." /></h3>
        <p className="group-description">Required to use GPT-4o and other OpenAI models. Get a key from platform.openai.com.</p>
        <input
          type="password"
          value={config.openai?.api_key || ''}
          onChange={(e) => setConfig({
            ...config,
            openai: { ...config.openai, api_key: e.target.value || undefined },
          })}
          placeholder="sk-..."
        />
        {config.openai?.api_key && (
          <input
            type="text"
            value={config.openai?.organization_id || ''}
            onChange={(e) => setConfig({
              ...config,
              openai: { ...config.openai, organization_id: e.target.value || undefined },
            })}
            placeholder="Organization ID (optional)"
          />
        )}
      </div>

      <CollapsibleSection title="Provider API Keys" defaultOpen={true}>
        <p className="group-description">Configure API keys for additional LLM providers.</p>

        <h4 style={{ margin: '8px 0 4px' }}>Cerebras</h4>
        <div className="settings-row">
          <label className="field">
            <span>API Key<InfoTip text="Cerebras Cloud API key for fast Llama/Qwen inference. Get one at cloud.cerebras.ai." /></span>
            <input type="password" value={config.cerebras?.api_key || ''} placeholder="csk-..."
              onChange={(e) => setConfig({ ...config, cerebras: { ...config.cerebras, api_key: e.target.value || undefined } })} />
          </label>
        </div>

        <h4 style={{ margin: '8px 0 4px' }}>Google Gemini</h4>
        <div className="settings-row">
          <label className="field">
            <span>API Key<InfoTip text="Google AI Studio API key for Gemini models. Get one at aistudio.google.com." /></span>
            <input type="password" value={config.google?.api_key || ''} placeholder="AIza..."
              onChange={(e) => setConfig({ ...config, google: { default_model: config.google?.default_model || 'gemini-2.0-flash', api_key: e.target.value || undefined } })} />
          </label>
          <label className="field">
            <span>Default Model<InfoTip text="Default Gemini model to use." /></span>
            <input type="text" value={config.google?.default_model || 'gemini-2.0-flash'} placeholder="gemini-2.0-flash"
              onChange={(e) => setConfig({ ...config, google: { ...config.google, default_model: e.target.value || 'gemini-2.0-flash' } })} />
          </label>
        </div>

        <h4 style={{ margin: '12px 0 4px' }}>DeepSeek</h4>
        <div className="settings-row">
          <label className="field">
            <span>API Key<InfoTip text="DeepSeek API key." /></span>
            <input type="password" value={config.deepseek?.api_key || ''} placeholder="sk-..."
              onChange={(e) => setConfig({ ...config, deepseek: { api_url: config.deepseek?.api_url || 'https://api.deepseek.com/v1', default_model: config.deepseek?.default_model || 'deepseek-chat', api_key: e.target.value || undefined } })} />
          </label>
        </div>
        <div className="settings-row">
          <label className="field">
            <span>API URL<InfoTip text="DeepSeek API endpoint URL." /></span>
            <input type="text" value={config.deepseek?.api_url || 'https://api.deepseek.com/v1'} placeholder="https://api.deepseek.com/v1"
              onChange={(e) => setConfig({ ...config, deepseek: { ...config.deepseek!, api_url: e.target.value || 'https://api.deepseek.com/v1' } })} />
          </label>
          <label className="field">
            <span>Default Model<InfoTip text="Default DeepSeek model to use." /></span>
            <input type="text" value={config.deepseek?.default_model || 'deepseek-chat'} placeholder="deepseek-chat"
              onChange={(e) => setConfig({ ...config, deepseek: { ...config.deepseek!, default_model: e.target.value || 'deepseek-chat' } })} />
          </label>
        </div>

        <h4 style={{ margin: '12px 0 4px' }}>GitHub Copilot</h4>
        <div className="settings-row">
          <label className="field">
            <span>Token<InfoTip text="GitHub Copilot authentication token." /></span>
            <input type="password" value={config.github_copilot?.token || ''} placeholder="ghu_..."
              onChange={(e) => setConfig({ ...config, github_copilot: { api_url: config.github_copilot?.api_url || 'https://api.githubcopilot.com', token: e.target.value || undefined } })} />
          </label>
          <label className="field">
            <span>API URL<InfoTip text="GitHub Copilot API endpoint." /></span>
            <input type="text" value={config.github_copilot?.api_url || 'https://api.githubcopilot.com'} placeholder="https://api.githubcopilot.com"
              onChange={(e) => setConfig({ ...config, github_copilot: { ...config.github_copilot!, api_url: e.target.value || 'https://api.githubcopilot.com' } })} />
          </label>
        </div>

        <h4 style={{ margin: '12px 0 4px' }}>MiniMax</h4>
        <div className="settings-row">
          <label className="field">
            <span>API Key<InfoTip text="MiniMax API key." /></span>
            <input type="password" value={config.minimax?.api_key || ''} placeholder="API key"
              onChange={(e) => setConfig({ ...config, minimax: { api_url: config.minimax?.api_url || 'https://api.minimax.chat/v1', default_model: config.minimax?.default_model || 'minimax-2.5', api_key: e.target.value || undefined } })} />
          </label>
        </div>
        <div className="settings-row">
          <label className="field">
            <span>API URL<InfoTip text="MiniMax API endpoint URL." /></span>
            <input type="text" value={config.minimax?.api_url || 'https://api.minimax.chat/v1'} placeholder="https://api.minimax.chat/v1"
              onChange={(e) => setConfig({ ...config, minimax: { ...config.minimax!, api_url: e.target.value || 'https://api.minimax.chat/v1' } })} />
          </label>
          <label className="field">
            <span>Default Model<InfoTip text="Default MiniMax model to use." /></span>
            <input type="text" value={config.minimax?.default_model || 'minimax-2.5'} placeholder="minimax-2.5"
              onChange={(e) => setConfig({ ...config, minimax: { ...config.minimax!, default_model: e.target.value || 'minimax-2.5' } })} />
          </label>
        </div>
      </CollapsibleSection>

      <CollapsibleSection title="Ollama (Local Models)">
        <p className="group-description">Connect to a local Ollama instance for self-hosted models.</p>
        <label className="field">
          <span>Ollama Status<InfoTip text="Whether Ollama is detected and available for model inference." /></span>
        </label>
        <button className="test-button" onClick={async () => {
          try {
            const models = await invoke<{ name: string; size: number | null; modified_at: string | null }[]>('discover_ollama_models');
            notifyInfo('Ollama', `Found ${models.length} model(s): ${models.map(m => m.name).join(', ') || 'none'}`);
          } catch (error) {
            notifyError('Ollama', `Ollama not available: ${error}`);
          }
        }}>
          Discover Ollama Models
        </button>
      </CollapsibleSection>
    </div>
  );
}
