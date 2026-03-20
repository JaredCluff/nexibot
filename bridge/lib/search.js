/**
 * DuckDuckGo search proxy router.
 *
 * Node.js fetch has a browser-like TLS fingerprint that avoids CAPTCHAs.
 * Rust's reqwest gets blocked by DuckDuckGo's bot detection.
 */

import { Router } from 'express';

const router = Router();

/**
 * POST /api/search
 * { "query": "search terms", "num_results": 10 }
 */
router.post('/api/search', async (req, res) => {
  const { query, num_results = 10 } = req.body;

  if (!query) {
    return res.status(400).json({ error: 'Missing query' });
  }

  console.log('[Bridge] DuckDuckGo search:', { query, num_results });

  try {
    const encoded = query.replace(/ /g, '+').replace(/[^\w+.-]/g, c =>
      '%' + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0')
    );

    const response = await fetch('https://html.duckduckgo.com/html/', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
        'Referer': 'https://duckduckgo.com/',
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
      },
      body: `q=${encoded}&b=&kl=&df=`,
    });

    if (!response.ok) {
      return res.status(502).json({ error: `DuckDuckGo returned ${response.status}` });
    }

    const html = await response.text();

    // Check for CAPTCHA
    if (html.includes('anomaly') && html.includes('botnet')) {
      return res.status(503).json({ error: 'DuckDuckGo returned CAPTCHA' });
    }

    // Parse results from HTML
    const results = [];
    const blocks = html.split('class="links_main');

    for (let i = 1; i < blocks.length && results.length < num_results; i++) {
      const block = blocks[i];

      // Extract URL from href before result__a
      const aPos = block.indexOf('class="result__a"');
      if (aPos === -1) continue;

      const before = block.substring(0, aPos);
      const hrefMatch = before.match(/href="([^"]+)"/g);
      if (!hrefMatch) continue;

      const lastHref = hrefMatch[hrefMatch.length - 1];
      let url = lastHref.replace('href="', '').replace('"', '');

      // Unwrap DuckDuckGo redirect
      const uddgIdx = url.indexOf('uddg=');
      if (uddgIdx !== -1) {
        const rawTarget = url.substring(uddgIdx + 5);
        const ampIdx = rawTarget.indexOf('&');
        url = decodeURIComponent(ampIdx !== -1 ? rawTarget.substring(0, ampIdx) : rawTarget);
      } else if (url.startsWith('//')) {
        url = 'https:' + url;
      }

      // Extract title
      const after = block.substring(aPos);
      const titleMatch = after.match(/class="result__a"[^>]*>([^<]*(?:<[^/][^>]*>[^<]*)*)<\/a>/);
      const title = titleMatch ? titleMatch[1].replace(/<[^>]+>/g, '').trim() : '';

      // Extract snippet
      const snippetMatch = block.match(/class="result__snippet"[^>]*>([^<]*(?:<[^/][^>]*>[^<]*)*)<\/(?:a|span|td)>/);
      const snippet = snippetMatch ? snippetMatch[1].replace(/<[^>]+>/g, '').trim() : '';

      if (title && url && !url.includes('duckduckgo.com')) {
        results.push({ title, url, snippet });
      }
    }

    console.log(`[Bridge] DuckDuckGo returned ${results.length} results`);
    res.json({ results });

  } catch (error) {
    console.error('[Bridge] DuckDuckGo search error:', error.message);
    res.status(500).json({ error: error.message });
  }
});

export default router;
