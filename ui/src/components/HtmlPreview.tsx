import { useState } from 'react';

interface HtmlPreviewProps {
  content: string;
  title?: string;
}

function HtmlPreview({ content, title }: HtmlPreviewProps) {
  const [showSource, setShowSource] = useState(false);

  return (
    <div className="html-preview">
      <div className="html-preview-toolbar">
        {title && <span className="html-preview-title">{title}</span>}
        <button
          className={`html-preview-toggle ${!showSource ? 'active' : ''}`}
          onClick={() => setShowSource(false)}
        >
          Preview
        </button>
        <button
          className={`html-preview-toggle ${showSource ? 'active' : ''}`}
          onClick={() => setShowSource(true)}
        >
          Source
        </button>
      </div>
      <div className="html-preview-content">
        {showSource ? (
          <pre className="html-preview-source">
            <code>{content}</code>
          </pre>
        ) : (
          <iframe
            className="html-preview-iframe"
            srcDoc={content}
            sandbox=""
            title={title || 'HTML Preview'}
          />
        )}
      </div>
    </div>
  );
}

export default HtmlPreview;
