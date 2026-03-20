// Integration test: Verify ChatGPT OAuth models endpoint works
const fs = require('fs');
const path = require('path');

async function main() {
  const profilePath = path.join(process.env.APPDATA, 'ai.nexibot.desktop/auth-profiles.json');

  if (!fs.existsSync(profilePath)) {
    console.log('SKIP: No auth profiles found at', profilePath);
    process.exit(0);
  }

  const profiles = JSON.parse(fs.readFileSync(profilePath, 'utf8'));
  const openaiProfile = profiles.find(p => p.provider === 'openai');

  if (!openaiProfile) {
    console.log('SKIP: No OpenAI profile found');
    process.exit(0);
  }

  const token = openaiProfile.access_token;
  const isChatGPTToken = token.startsWith('eyJ');
  console.log('1. Token detected as ChatGPT JWT:', isChatGPTToken);

  if (!isChatGPTToken) {
    console.error('FAIL: Expected JWT token starting with eyJ');
    process.exit(1);
  }

  // Test ChatGPT models endpoint (same as bridge server.js logic)
  console.log('2. Calling https://chatgpt.com/backend-api/codex/models...');
  const response = await fetch('https://chatgpt.com/backend-api/codex/models?client_version=0.99.0', {
    headers: { 'Authorization': `Bearer ${token}` },
  });

  console.log('   Status:', response.status);

  if (!response.ok) {
    const err = await response.text();
    console.error('FAIL: API returned', response.status, err);
    process.exit(1);
  }

  const data = await response.json();
  const models = (data.models || [])
    .filter(m => m.supported_in_api)
    .map(m => ({ id: m.slug, display_name: m.display_name || m.slug }))
    .sort((a, b) => a.id.localeCompare(b.id));

  console.log('3. Models returned:', models.length);
  models.forEach(m => console.log(`   - ${m.id} (${m.display_name})`));

  if (models.length === 0) {
    console.error('FAIL: API returned 0 models');
    process.exit(1);
  }

  console.log('\nPASS: ChatGPT models API returned', models.length, 'models via API');
}

main().catch(e => { console.error('FAIL:', e.message); process.exit(1); });
