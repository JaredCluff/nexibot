import { useState, useMemo } from 'react';
import DOMPurify from 'dompurify';

interface SvgPreviewProps {
  content: string;
  title?: string;
}

/** Sanitize SVG using DOMPurify with SVG-specific rules. */
function sanitizeSvg(svg: string): string {
  return DOMPurify.sanitize(svg, {
    USE_PROFILES: { svg: true, svgFilters: true },
    // Forbid foreignObject (can embed arbitrary HTML) and use (can load external resources)
    FORBID_TAGS: ['script', 'foreignObject'],
    FORBID_ATTR: ['xlink:href', 'formaction', 'action'],
  });
}

function SvgPreview({ content, title }: SvgPreviewProps) {
  const [showSource, setShowSource] = useState(false);

  const sanitized = useMemo(() => sanitizeSvg(content), [content]);

  return (
    <div className="svg-preview">
      <div className="svg-preview-toolbar">
        {title && <span className="svg-preview-title">{title}</span>}
        <button
          className={`svg-preview-toggle ${!showSource ? 'active' : ''}`}
          onClick={() => setShowSource(false)}
        >
          Preview
        </button>
        <button
          className={`svg-preview-toggle ${showSource ? 'active' : ''}`}
          onClick={() => setShowSource(true)}
        >
          Source
        </button>
      </div>
      <div className="svg-preview-content">
        {showSource ? (
          <pre className="svg-preview-source">
            <code>{content}</code>
          </pre>
        ) : (
          <div
            className="svg-preview-rendered"
            dangerouslySetInnerHTML={{ __html: sanitized }}
          />
        )}
      </div>
    </div>
  );
}

export default SvgPreview;
