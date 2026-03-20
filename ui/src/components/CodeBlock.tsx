import { useState, useCallback } from 'react';

interface CodeBlockProps {
  code: string;
  language?: string;
  onOpenInCanvas?: (code: string, language: string) => void;
}

function CodeBlock({ code, language = '', onOpenInCanvas }: CodeBlockProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback for environments without clipboard API
      const textarea = document.createElement('textarea');
      textarea.value = code;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }, [code]);

  const handleOpenInCanvas = useCallback(() => {
    onOpenInCanvas?.(code, language);
  }, [code, language, onOpenInCanvas]);

  return (
    <div className="codeblock">
      <div className="codeblock-header">
        {language && <span className="codeblock-lang">{language}</span>}
        <div className="codeblock-actions">
          {onOpenInCanvas && (
            <button
              className="codeblock-btn"
              onClick={handleOpenInCanvas}
              title="Open in Canvas"
            >
              Open in Canvas
            </button>
          )}
          <button
            className="codeblock-btn"
            onClick={handleCopy}
            title="Copy code"
          >
            {copied ? 'Copied!' : 'Copy'}
          </button>
        </div>
      </div>
      <pre className="codeblock-pre">
        <code className={language ? `language-${language}` : ''}>{code}</code>
      </pre>
    </div>
  );
}

export default CodeBlock;
