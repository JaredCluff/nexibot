import { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-shell';
import './Onboarding.css';

interface OnboardingProps {
  onComplete: () => void;
}

type Step =
  | 'welcome'
  | 'choose-auth'
  | 'choose-signin-provider'
  | 'choose-apikey-provider'
  | 'oauth-signin'
  | 'api-key-claude'
  | 'api-key-openai'
  | 'api-key-cerebras'
  | 'openai-device-code'
  | 'knexus-subscription'
  | 'complete';

interface NexiBotConfig {
  claude: {
    api_key?: string;
    model: string;
    max_tokens: number;
    system_prompt: string;
  };
  openai: {
    api_key?: string;
    model?: string;
  };
  cerebras?: {
    api_key?: string;
    model?: string;
  };
  k2k: {
    enabled: boolean;
    local_agent_url: string;
    router_url?: string;
    private_key_pem?: string;
    client_id: string;
  };
  audio: {
    enabled: boolean;
    input_device?: string;
    sample_rate: number;
    channels: number;
  };
  wakeword: {
    enabled: boolean;
    wake_word: string;
    threshold: number;
    model_path?: string;
  };
}

interface DeviceFlowResponse {
  user_code: string;
  verification_uri: string;
  interval: number;
}

interface DeviceFlowPollResult {
  status: 'pending' | 'complete' | 'expired' | 'denied';
  error: string | null;
}

function Onboarding({ onComplete }: OnboardingProps) {
  const [step, setStep] = useState<Step>('welcome');
  const [apiKey, setApiKey] = useState('');
  const [oauthCode, setOauthCode] = useState('');
  const [waitingForCode, setWaitingForCode] = useState(false);
  const [authUrl, setAuthUrl] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  // OpenAI device code flow state
  const [deviceCode, setDeviceCode] = useState<DeviceFlowResponse | null>(null);
  const [deviceCodeCopied, setDeviceCodeCopied] = useState(false);
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Knowledge Nexus subscription state
  const [knexusStatus, setKnexusStatus] = useState<string | null>(null);

  // Cleanup polling on unmount
  useEffect(() => {
    return () => {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current);
      }
    };
  }, []);

  // --- Claude OAuth ---

  const handleOAuthOpenBrowser = async () => {
    setLoading(true);
    setError('');
    try {
      const url = await invoke<string>('open_oauth_browser', { provider: 'anthropic' });
      setAuthUrl(url);
      setWaitingForCode(true);
      setLoading(false);
    } catch (e) {
      setError(`Failed to start sign-in: ${e}`);
      setLoading(false);
    }
  };

  const handleOAuthCodeSubmit = async () => {
    if (!oauthCode.trim()) { setError('Please paste the authorization code'); return; }
    setLoading(true);
    setError('');
    try {
      await invoke('complete_oauth_flow', { provider: 'anthropic', code: oauthCode.trim() });
      await finalizeSetup('claude');
    } catch (e) {
      setError(`Authentication failed: ${e}`);
      setLoading(false);
    }
  };

  // --- API Key handlers ---

  const handleClaudeApiKey = async () => {
    if (!apiKey.trim()) { setError('Please enter a valid API key'); return; }
    if (!apiKey.startsWith('sk-ant-')) { setError('Invalid format. Anthropic keys start with "sk-ant-"'); return; }
    setLoading(true);
    setError('');
    try {
      const config = await invoke<NexiBotConfig>('get_config');
      config.claude.api_key = apiKey;
      config.k2k.enabled = true;
      await invoke('update_config', { newConfig: config });
      await finalizeSetup('claude');
    } catch (e) {
      setError(`Failed to save API key: ${e}`);
      setLoading(false);
    }
  };

  const handleOpenAIApiKey = async () => {
    if (!apiKey.trim()) { setError('Please enter a valid API key'); return; }
    if (apiKey.startsWith('sk-ant-')) { setError('This looks like an Anthropic key. Go back and choose Anthropic instead.'); return; }
    if (!apiKey.startsWith('sk-')) { setError('Invalid format. OpenAI keys start with "sk-"'); return; }
    setLoading(true);
    setError('');
    try {
      const config = await invoke<NexiBotConfig>('get_config');
      config.openai.api_key = apiKey;
      config.k2k.enabled = true;
      await invoke('update_config', { newConfig: config });
      await finalizeSetup('openai');
    } catch (e) {
      setError(`Failed to save API key: ${e}`);
      setLoading(false);
    }
  };

  const handleCerebrasApiKey = async () => {
    if (!apiKey.trim()) { setError('Please enter a valid API key'); return; }
    if (!apiKey.startsWith('csk-')) { setError('Invalid format. Cerebras keys start with "csk-"'); return; }
    setLoading(true);
    setError('');
    try {
      const config = await invoke<NexiBotConfig>('get_config');
      if (!config.cerebras) config.cerebras = {};
      config.cerebras.api_key = apiKey;
      config.k2k.enabled = true;
      await invoke('update_config', { newConfig: config });

      // Validate models and find the largest one
      try {
        const validated = await invoke<Array<{ id: string; size_score: number }>>('validate_provider_models', { provider: 'Cerebras' });
        if (validated.length > 0) {
          // Already sorted by size_score descending
          config.claude.model = validated[0].id;
          await invoke('update_config', { newConfig: config });
        }
      } catch (_) { /* non-fatal — will use default */ }

      await finalizeSetup('cerebras');
    } catch (e) {
      setError(`Failed to save API key: ${e}`);
      setLoading(false);
    }
  };

  // --- OpenAI device code flow ---

  const startDeviceCodeFlow = async () => {
    setLoading(true);
    setError('');
    try {
      const response = await invoke<DeviceFlowResponse>('start_openai_device_flow');
      setDeviceCode(response);
      setLoading(false);

      // Open the verification page in browser
      open(response.verification_uri).catch(() => {});

      const interval = Math.max(response.interval, 5) * 1000;
      pollIntervalRef.current = setInterval(async () => {
        try {
          const result = await invoke<DeviceFlowPollResult>('poll_openai_device_flow');
          if (result.status === 'complete') {
            if (pollIntervalRef.current) clearInterval(pollIntervalRef.current);
            await finalizeSetup('openai');
          } else if (result.status === 'expired' || result.status === 'denied') {
            if (pollIntervalRef.current) clearInterval(pollIntervalRef.current);
            setError(result.error || `Device code flow ${result.status}.`);
            setDeviceCode(null);
          }
        } catch (e) {
          if (pollIntervalRef.current) clearInterval(pollIntervalRef.current);
          setError(`Polling failed: ${e}`);
          setDeviceCode(null);
        }
      }, interval);
    } catch (e) {
      setError(`Failed to start device code flow: ${e}`);
      setLoading(false);
    }
  };

  const copyDeviceCode = () => {
    if (deviceCode) {
      navigator.clipboard.writeText(deviceCode.user_code);
      setDeviceCodeCopied(true);
      setTimeout(() => setDeviceCodeCopied(false), 2000);
    }
  };

  // --- Knowledge Nexus subscription ---

  const checkKnexusSubscription = async (provider: 'anthropic' | 'openai') => {
    setLoading(true);
    setError('');
    setKnexusStatus('checking');
    try {
      const status = await invoke<'Active' | 'Inactive' | 'Expired' | 'Pending'>('check_subscription', { provider });
      if (status === 'Active') {
        setKnexusStatus('provisioning');
        const apiKeyResult = await invoke<string>('get_subscription_credentials', { provider });

        // Save provisioned key to config
        const config = await invoke<NexiBotConfig>('get_config');
        if (provider === 'anthropic') {
          config.claude.api_key = apiKeyResult;
        } else {
          config.openai.api_key = apiKeyResult;
        }
        config.k2k.enabled = true;
        await invoke('update_config', { newConfig: config });

        setKnexusStatus('complete');
        await finalizeSetup(provider === 'anthropic' ? 'claude' : 'openai');
      } else if (status === 'Pending') {
        setKnexusStatus('pending');
        setError('Your subscription is pending activation. Please check back shortly.');
        setLoading(false);
      } else if (status === 'Expired') {
        setKnexusStatus('expired');
        setError('Your subscription has expired. Please renew at the subscription portal.');
        setLoading(false);
      } else {
        setKnexusStatus('inactive');
        setLoading(false);
      }
    } catch (e) {
      setKnexusStatus('error');
      setError(`Failed to check subscription: ${e}`);
      setLoading(false);
    }
  };

  const openSubscriptionPortal = async () => {
    try {
      await invoke('open_subscription_portal', { provider: null });
    } catch (_) {
      /* subscription portal not configured */
    }
  };

  // --- Finalize ---

  const finalizeSetup = async (provider: 'claude' | 'openai' | 'cerebras') => {
    try {
      const config = await invoke<NexiBotConfig>('get_config');
      let changed = false;
      if (!config.k2k.enabled) {
        config.k2k.enabled = true;
        changed = true;
      }
      if (provider === 'openai') {
        config.claude.model = 'gpt-4o';
        if (!config.openai.model) {
          config.openai.model = 'gpt-4o';
        }
        changed = true;
      }
      if (provider === 'cerebras') {
        config.claude.model = 'cerebras/gpt-oss-120b';
        if (!config.cerebras) config.cerebras = {};
        changed = true;
      }
      if (changed) {
        await invoke('update_config', { newConfig: config });
      }
      try { await invoke('ensure_bridge_running'); } catch (_) { /* non-fatal */ }

      setStep('complete');
      setLoading(false);
      setTimeout(() => { onComplete(); }, 2000);
    } catch (e) {
      setError(`Setup failed: ${e}`);
      setLoading(false);
    }
  };

  // --- Step indicator logic ---

  const getStepIndex = (): number => {
    if (step === 'welcome') return 0;
    if (step === 'choose-auth') return 1;
    if (step === 'choose-signin-provider' || step === 'choose-apikey-provider') return 2;
    if (step === 'complete') return 4;
    return 3; // all auth-specific flows
  };

  // --- Render ---

  const renderStep = () => {
    switch (step) {
      case 'welcome':
        return (
          <div className="onboarding-step">
            <h1>Welcome to NexiBot!</h1>
            <p className="subtitle">
              Your AI-powered conversational assistant with access to local and federated knowledge.
            </p>
            <div className="features">
              <div className="feature">
                <span className="icon">💬</span>
                <div>
                  <h3>Natural Conversations</h3>
                  <p>Chat naturally with AI</p>
                </div>
              </div>
              <div className="feature">
                <span className="icon">🔍</span>
                <div>
                  <h3>Local Knowledge Search</h3>
                  <p>Search your files via K2K protocol</p>
                </div>
              </div>
              <div className="feature">
                <span className="icon">🎙️</span>
                <div>
                  <h3>Voice Interaction</h3>
                  <p>Coming soon: Voice and wake word support</p>
                </div>
              </div>
            </div>
            <button className="primary" onClick={() => setStep('choose-auth')}>
              Get Started
            </button>
          </div>
        );

      case 'choose-auth':
        return (
          <div className="onboarding-step">
            <h2>How would you like to connect?</h2>
            <p>Choose your authentication method</p>

            <div className="provider-options">
              <button
                className="provider-option recommended"
                onClick={() => setStep('choose-signin-provider')}
              >
                <div className="provider-icon">⚡</div>
                <h3>Sign In</h3>
                <p>Use your existing subscription — Claude Pro/Max, ChatGPT Plus/Pro, or Knowledge Nexus</p>
                <span className="badge recommended-badge">Recommended</span>
              </button>

              <button
                className="provider-option"
                onClick={() => setStep('choose-apikey-provider')}
              >
                <div className="provider-icon">🔑</div>
                <h3>API Key</h3>
                <p>Enter an API key from your provider</p>
              </button>
            </div>

            <button className="secondary" onClick={() => setStep('welcome')}>
              Back
            </button>
          </div>
        );

      case 'choose-signin-provider':
        return (
          <div className="onboarding-step">
            <h2>Choose Your Provider</h2>
            <p>Sign in with your existing subscription</p>

            <div className="provider-options">
              <button
                className="provider-option"
                onClick={() => setStep('oauth-signin')}
              >
                <div className="provider-icon">⚡</div>
                <h3>Claude (Anthropic)</h3>
                <p>Claude Pro or Max subscription</p>
              </button>

              <button
                className="provider-option"
                onClick={() => { setDeviceCode(null); setError(''); setStep('openai-device-code'); }}
              >
                <div className="provider-icon">🤖</div>
                <h3>ChatGPT (OpenAI)</h3>
                <p>ChatGPT Plus, Pro, or Max subscription</p>
              </button>

              <button
                className="provider-option"
                onClick={() => { setKnexusStatus(null); setError(''); setStep('knexus-subscription'); }}
              >
                <div className="provider-icon">🌐</div>
                <h3>Knowledge Nexus</h3>
                <p>Use your Knowledge Nexus subscription</p>
              </button>
            </div>

            <button className="secondary" onClick={() => setStep('choose-auth')}>
              Back
            </button>
          </div>
        );

      case 'choose-apikey-provider':
        return (
          <div className="onboarding-step">
            <h2>Choose Your Provider</h2>
            <p>Enter an API key from your provider</p>

            <div className="provider-options">
              <button
                className="provider-option"
                onClick={() => { setApiKey(''); setError(''); setStep('api-key-claude'); }}
              >
                <div className="provider-icon">⚡</div>
                <h3>Anthropic</h3>
                <p>API key from console.anthropic.com</p>
              </button>

              <button
                className="provider-option"
                onClick={() => { setApiKey(''); setError(''); setStep('api-key-openai'); }}
              >
                <div className="provider-icon">🤖</div>
                <h3>OpenAI</h3>
                <p>API key from platform.openai.com</p>
              </button>

              <button
                className="provider-option"
                onClick={() => { setApiKey(''); setError(''); setStep('api-key-cerebras'); }}
              >
                <div className="provider-icon">🧠</div>
                <h3>Cerebras</h3>
                <p>Fast inference with Llama & GPT-OSS models. Free tier available.</p>
              </button>

              <button
                className="provider-option"
                onClick={() => { setKnexusStatus(null); setError(''); setStep('knexus-subscription'); }}
              >
                <div className="provider-icon">🌐</div>
                <h3>Knowledge Nexus</h3>
                <p>Use your Knowledge Nexus subscription</p>
              </button>
            </div>

            <button className="secondary" onClick={() => setStep('choose-auth')}>
              Back
            </button>
          </div>
        );

      case 'oauth-signin':
        return (
          <div className="onboarding-step">
            <h2>Sign in with Claude</h2>
            <p>Use your existing Claude Pro or Claude Max subscription</p>

            {error && <div className="error-message">{error}</div>}

            <div className="form">
              {!waitingForCode ? (
                <>
                  <button
                    className="primary signin-button"
                    onClick={handleOAuthOpenBrowser}
                    disabled={loading}
                    style={{ marginBottom: '20px' }}
                  >
                    {loading ? (
                      <><span className="spinner"></span> Opening browser...</>
                    ) : (
                      <><span className="anthropic-logo">⚡</span> Sign in with Browser</>
                    )}
                  </button>

                  <div className="help-text" style={{ padding: '15px', borderRadius: '8px', marginBottom: '20px' }}>
                    <p><strong>What happens:</strong></p>
                    <ol style={{ marginLeft: '20px', marginTop: '10px' }}>
                      <li>Your browser opens to Claude's sign-in page</li>
                      <li>Log in with your Claude Pro/Max account</li>
                      <li>Authorize NexiBot access</li>
                      <li>Copy the code shown and paste it below</li>
                    </ol>
                  </div>
                </>
              ) : (
                <>
                  <div className="help-text" style={{ padding: '12px', borderRadius: '8px', marginBottom: '16px' }}>
                    <p>A browser window should have opened to Claude's sign-in page.</p>
                    {authUrl && (
                      <p style={{ marginTop: '8px' }}>
                        Browser didn't open? <a href={authUrl} target="_blank" rel="noopener noreferrer" style={{ color: 'var(--primary)' }}>Click here to sign in manually</a>
                      </p>
                    )}
                  </div>

                  <label>
                    Authorization Code
                    <input
                      type="text"
                      value={oauthCode}
                      onChange={(e) => setOauthCode(e.target.value)}
                      placeholder="Paste the code from the browser..."
                      disabled={loading}
                      autoFocus
                    />
                  </label>

                  <div className="help-text" style={{ marginBottom: '20px' }}>
                    <p>After signing in, copy the code shown and paste it above. It looks like: <code>code#state</code></p>
                  </div>

                  <button
                    className="primary"
                    onClick={handleOAuthCodeSubmit}
                    disabled={loading || !oauthCode}
                  >
                    {loading ? 'Authenticating...' : 'Complete Sign In'}
                  </button>
                </>
              )}

              <button
                className="secondary"
                onClick={() => { setWaitingForCode(false); setOauthCode(''); setError(''); setStep('choose-signin-provider'); }}
                disabled={loading}
              >
                Back
              </button>
            </div>
          </div>
        );

      case 'api-key-claude':
        return (
          <div className="onboarding-step">
            <h2>Enter Your Anthropic API Key</h2>
            <p>Get your API key from <a href="https://console.anthropic.com" target="_blank" rel="noopener noreferrer">console.anthropic.com</a></p>

            {error && <div className="error-message">{error}</div>}

            <div className="form">
              <label>
                API Key
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="sk-ant-..."
                  disabled={loading}
                  autoFocus
                />
              </label>
              <div className="help-text">
                <p>Your API key is stored securely on your device and never shared.</p>
              </div>
              <button className="primary" onClick={handleClaudeApiKey} disabled={loading || !apiKey}>
                {loading ? 'Saving...' : 'Continue'}
              </button>
              <button className="secondary" onClick={() => { setError(''); setStep('choose-apikey-provider'); }} disabled={loading}>
                Back
              </button>
            </div>
          </div>
        );

      case 'api-key-openai':
        return (
          <div className="onboarding-step">
            <h2>Enter Your OpenAI API Key</h2>
            <p>Get your API key from <a href="https://platform.openai.com/api-keys" target="_blank" rel="noopener noreferrer">platform.openai.com</a></p>

            {error && <div className="error-message">{error}</div>}

            <div className="form">
              <label>
                API Key
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="sk-..."
                  disabled={loading}
                  autoFocus
                />
              </label>
              <div className="help-text">
                <p>Your API key is stored securely on your device and never shared.</p>
              </div>
              <button className="primary" onClick={handleOpenAIApiKey} disabled={loading || !apiKey}>
                {loading ? 'Saving...' : 'Continue'}
              </button>
              <button className="secondary" onClick={() => { setError(''); setStep('choose-apikey-provider'); }} disabled={loading}>
                Back
              </button>
            </div>
          </div>
        );

      case 'api-key-cerebras':
        return (
          <div className="onboarding-step">
            <h2>Enter Your Cerebras API Key</h2>
            <p>Get your API key from <a href="https://cloud.cerebras.ai" target="_blank" rel="noopener noreferrer">cloud.cerebras.ai</a></p>

            {error && <div className="error-message">{error}</div>}

            <div className="form">
              <label>
                API Key
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="csk-..."
                  disabled={loading}
                  autoFocus
                />
              </label>
              <div className="help-text">
                <p>Your API key is stored securely on your device and never shared. Cerebras offers a free tier for getting started.</p>
              </div>
              <button className="primary" onClick={handleCerebrasApiKey} disabled={loading || !apiKey}>
                {loading ? 'Validating models...' : 'Continue'}
              </button>
              <button className="secondary" onClick={() => { setError(''); setStep('choose-apikey-provider'); }} disabled={loading}>
                Back
              </button>
            </div>
          </div>
        );

      case 'openai-device-code':
        return (
          <div className="onboarding-step">
            <h2>Sign in with ChatGPT</h2>
            <p>Use your ChatGPT Plus, Pro, or Max subscription</p>

            {error && <div className="error-message">{error}</div>}

            {!deviceCode ? (
              <div className="form">
                <div className="oauth-info">
                  <div className="benefit-list">
                    <div className="benefit-item"><span className="check-icon">✓</span><span>No API key needed</span></div>
                    <div className="benefit-item"><span className="check-icon">✓</span><span>Use your existing ChatGPT subscription</span></div>
                    <div className="benefit-item"><span className="check-icon">✓</span><span>Works with ChatGPT Plus, Pro & Max</span></div>
                  </div>
                </div>
                <button className="primary" onClick={startDeviceCodeFlow} disabled={loading}>
                  {loading ? 'Starting...' : 'Start Sign In'}
                </button>
                <button className="secondary" onClick={() => { setError(''); setStep('choose-signin-provider'); }} disabled={loading}>
                  Back
                </button>
              </div>
            ) : (
              <div className="form">
                <div className="help-text" style={{ padding: '15px', borderRadius: '8px', marginBottom: '8px' }}>
                  <p><strong>Enter this code in your browser:</strong></p>
                </div>

                <div className="device-code-display" onClick={copyDeviceCode} title="Click to copy">
                  {deviceCode.user_code}
                </div>

                <p style={{ fontSize: '14px', color: 'var(--text-secondary)', marginBottom: '16px' }}>
                  {deviceCodeCopied ? 'Copied!' : 'Click the code to copy it'}
                </p>

                <div className="help-text" style={{ padding: '15px', borderRadius: '8px', marginBottom: '20px' }}>
                  <p><strong>Steps:</strong></p>
                  <ol style={{ marginLeft: '20px', marginTop: '10px' }}>
                    <li>A browser window should have opened automatically</li>
                    <li>Enter the code above on the OpenAI page</li>
                    <li>Sign in with your ChatGPT account</li>
                    <li>NexiBot will detect completion automatically</li>
                  </ol>
                  <p style={{ marginTop: '12px' }}>
                    Browser didn't open? <a href={deviceCode.verification_uri} target="_blank" rel="noopener noreferrer" style={{ color: 'var(--primary)' }}>Click here</a>
                  </p>
                </div>

                <div className="loading-spinner"></div>
                <p style={{ fontSize: '13px', color: 'var(--text-secondary)' }}>Waiting for authorization...</p>

                <button className="secondary" onClick={() => {
                  if (pollIntervalRef.current) clearInterval(pollIntervalRef.current);
                  setDeviceCode(null); setError(''); setStep('choose-signin-provider');
                }}>
                  Cancel
                </button>
              </div>
            )}
          </div>
        );

      case 'knexus-subscription':
        return (
          <div className="onboarding-step">
            <h2>Knowledge Nexus</h2>
            <p>Use your Knowledge Nexus subscription for automatic API provisioning</p>

            {error && <div className="error-message">{error}</div>}

            <div className="form">
              {(!knexusStatus || knexusStatus === 'inactive' || knexusStatus === 'expired' || knexusStatus === 'error') && (
                <>
                  <div className="oauth-info">
                    <div className="benefit-list">
                      <div className="benefit-item"><span className="check-icon">✓</span><span>No manual API key entry</span></div>
                      <div className="benefit-item"><span className="check-icon">✓</span><span>Credentials provisioned automatically</span></div>
                      <div className="benefit-item"><span className="check-icon">✓</span><span>Supports Claude and GPT-4o</span></div>
                    </div>
                  </div>

                  {knexusStatus === 'inactive' && (
                    <div className="help-text" style={{ padding: '15px', borderRadius: '8px', marginBottom: '16px' }}>
                      <p><strong>No active subscription found.</strong></p>
                      <p style={{ marginTop: '8px' }}>Subscribe at the Knowledge Nexus portal, then click "Check Again" below.</p>
                    </div>
                  )}

                  <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
                    <p style={{ fontSize: '14px', color: 'var(--text-secondary)', margin: '0' }}>Which AI provider should we provision?</p>
                    <div style={{ display: 'flex', gap: '12px' }}>
                      <button
                        className="primary"
                        onClick={() => checkKnexusSubscription('anthropic')}
                        disabled={loading}
                        style={{ flex: 1 }}
                      >
                        {loading ? 'Checking...' : 'Claude (Anthropic)'}
                      </button>
                      <button
                        className="primary"
                        onClick={() => checkKnexusSubscription('openai')}
                        disabled={loading}
                        style={{ flex: 1 }}
                      >
                        {loading ? 'Checking...' : 'GPT-4o (OpenAI)'}
                      </button>
                    </div>
                  </div>

                  {(knexusStatus === 'inactive' || knexusStatus === 'expired') && (
                    <button className="secondary" onClick={openSubscriptionPortal} style={{ marginTop: '8px' }}>
                      Open Subscription Portal
                    </button>
                  )}

                  <button className="secondary" onClick={() => { setError(''); setKnexusStatus(null); setStep('choose-auth'); }} disabled={loading}>
                    Back
                  </button>
                </>
              )}

              {(knexusStatus === 'checking' || knexusStatus === 'provisioning') && (
                <div style={{ textAlign: 'center', padding: '20px' }}>
                  <div className="loading-spinner"></div>
                  <p style={{ fontSize: '14px', color: 'var(--text-secondary)' }}>
                    {knexusStatus === 'checking' ? 'Checking subscription...' : 'Provisioning credentials...'}
                  </p>
                </div>
              )}

              {knexusStatus === 'pending' && (
                <>
                  <div className="help-text" style={{ padding: '15px', borderRadius: '8px' }}>
                    <p><strong>Subscription is pending activation.</strong></p>
                    <p style={{ marginTop: '8px' }}>Please wait a moment and try again.</p>
                  </div>
                  <button className="primary" onClick={() => { setKnexusStatus(null); setError(''); }}>
                    Try Again
                  </button>
                  <button className="secondary" onClick={() => { setError(''); setKnexusStatus(null); setStep('choose-auth'); }}>
                    Back
                  </button>
                </>
              )}
            </div>
          </div>
        );

      case 'complete':
        return (
          <div className="onboarding-step">
            <div className="success-icon">✨</div>
            <h2>All Set!</h2>
            <p>NexiBot is ready to use. Starting your first conversation...</p>
            <div className="loading-spinner"></div>
          </div>
        );

      default:
        return null;
    }
  };

  const currentStepIndex = getStepIndex();

  return (
    <div className="onboarding-overlay">
      <div className="onboarding-container">
        <div className="onboarding-content">
          {renderStep()}
        </div>

        <div className="onboarding-footer">
          <div className="step-indicators">
            {[0, 1, 2, 3, 4].map(i => (
              <span key={i} className={i === currentStepIndex ? 'active' : ''} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

export default Onboarding;
