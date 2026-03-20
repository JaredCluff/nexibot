import { useState, useEffect, useRef, useCallback } from 'react';
import DOMPurify from 'dompurify';

interface MermaidDiagramProps {
  code: string;
  title?: string;
}

function MermaidDiagram({ code, title }: MermaidDiagramProps) {
  const [svgContent, setSvgContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const containerRef = useRef<HTMLDivElement>(null);
  const idRef = useRef(`mermaid-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`);

  const renderDiagram = useCallback(async () => {
    setLoading(true);
    setError(null);
    setSvgContent(null);

    try {
      // Dynamic import of mermaid - uses variable to prevent Rollup from
      // treating it as a hard dependency. Fails gracefully if not installed.
      const mermaidModule = 'mermaid';
      const mermaid = await (Function('m', 'return import(m)')(mermaidModule)) as { default: any };
      const mermaidApi = mermaid.default;

      mermaidApi.initialize({
        startOnLoad: false,
        theme: 'dark',
        securityLevel: 'antiscript',
      });

      const { svg } = await mermaidApi.render(idRef.current, code);
      // Sanitize the rendered SVG output as a second defence layer
      const safe = DOMPurify.sanitize(svg, {
        USE_PROFILES: { svg: true, svgFilters: true },
        FORBID_TAGS: ['script', 'foreignObject'],
      });
      setSvgContent(safe);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      // Check if it is a module-not-found error vs a diagram syntax error
      if (message.includes('Failed to fetch') || message.includes('Module not found') || message.includes('Cannot find module') || message.includes('Failed to resolve')) {
        setError('Mermaid library not available. Install it with: npm install mermaid');
      } else {
        setError(`Diagram error: ${message}`);
      }
    } finally {
      setLoading(false);
    }
  }, [code]);

  useEffect(() => {
    renderDiagram();
  }, [renderDiagram]);

  if (loading) {
    return (
      <div className="mermaid-diagram">
        {title && <div className="mermaid-title">{title}</div>}
        <div className="mermaid-loading">
          <div className="mermaid-spinner"></div>
          <span>Rendering diagram...</span>
        </div>
      </div>
    );
  }

  if (error || !svgContent) {
    return (
      <div className="mermaid-diagram">
        {title && <div className="mermaid-title">{title}</div>}
        {error && <div className="mermaid-error">{error}</div>}
        <div className="mermaid-fallback">
          <div className="mermaid-fallback-label">Mermaid Source</div>
          <pre className="mermaid-fallback-code">
            <code>{code}</code>
          </pre>
        </div>
      </div>
    );
  }

  return (
    <div className="mermaid-diagram">
      {title && <div className="mermaid-title">{title}</div>}
      <div
        className="mermaid-rendered"
        ref={containerRef}
        dangerouslySetInnerHTML={{ __html: svgContent }}
      />
    </div>
  );
}

export default MermaidDiagram;
