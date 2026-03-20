import { useState, useEffect } from 'react';
import CodeBlock from './CodeBlock';
import HtmlPreview from './HtmlPreview';
import MermaidDiagram from './MermaidDiagram';
import SvgPreview from './SvgPreview';
import './Canvas.css';

export interface Artifact {
  id: string;
  type: 'code' | 'html' | 'svg' | 'mermaid';
  language?: string;
  content: string;
  title: string;
}

interface CanvasProps {
  artifacts: Artifact[];
  onClose: () => void;
  onRemoveArtifact: (id: string) => void;
}

function Canvas({ artifacts, onClose, onRemoveArtifact }: CanvasProps) {
  const [activeTab, setActiveTab] = useState<string | null>(
    artifacts.length > 0 ? artifacts[0].id : null
  );

  // If activeTab refers to a removed artifact, snap to first available.
  // useEffect is the correct place for this state sync — not during render with setTimeout.
  useEffect(() => {
    if (activeTab && !artifacts.find((a) => a.id === activeTab)) {
      setActiveTab(artifacts.length > 0 ? artifacts[0].id : null);
    }
  }, [artifacts, activeTab]);

  const activeArtifact = artifacts.find((a) => a.id === activeTab);

  const handleTabClick = (id: string) => {
    setActiveTab(id);
  };

  const handleCloseTab = (e: React.MouseEvent, id: string) => {
    e.stopPropagation();
    onRemoveArtifact(id);
    if (activeTab === id) {
      const remaining = artifacts.filter((a) => a.id !== id);
      setActiveTab(remaining.length > 0 ? remaining[0].id : null);
    }
  };

  const renderArtifact = (artifact: Artifact) => {
    switch (artifact.type) {
      case 'code':
        return (
          <CodeBlock
            code={artifact.content}
            language={artifact.language}
          />
        );
      case 'html':
        return <HtmlPreview content={artifact.content} title={artifact.title} />;
      case 'svg':
        return <SvgPreview content={artifact.content} title={artifact.title} />;
      case 'mermaid':
        return <MermaidDiagram code={artifact.content} title={artifact.title} />;
      default:
        return (
          <pre className="canvas-fallback">
            <code>{artifact.content}</code>
          </pre>
        );
    }
  };

  return (
    <div className="canvas">
      <div className="canvas-header">
        <span className="canvas-title">Canvas</span>
        <button className="canvas-close-btn" onClick={onClose} title="Close canvas">
          &times;
        </button>
      </div>

      {artifacts.length === 0 ? (
        <div className="canvas-empty">
          <p>No artifacts yet.</p>
          <p className="canvas-empty-hint">
            Click "Open in Canvas" on a code block to add it here.
          </p>
        </div>
      ) : (
        <>
          <div className="canvas-tabs">
            {artifacts.map((artifact) => (
              <div
                key={artifact.id}
                className={`canvas-tab ${artifact.id === activeTab ? 'active' : ''}`}
                onClick={() => handleTabClick(artifact.id)}
              >
                <span className="canvas-tab-label">{artifact.title}</span>
                <button
                  className="canvas-tab-close"
                  onClick={(e) => handleCloseTab(e, artifact.id)}
                  title="Close tab"
                >
                  &times;
                </button>
              </div>
            ))}
          </div>

          <div className="canvas-content">
            {activeArtifact && renderArtifact(activeArtifact)}
          </div>
        </>
      )}
    </div>
  );
}

export default Canvas;
