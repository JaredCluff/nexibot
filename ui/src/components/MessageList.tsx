import { useRef, useEffect, useCallback } from 'react';
import ReactMarkdown from 'react-markdown';
import CodeBlock from './CodeBlock';
import ToolStatusStrip from './ToolStatusStrip';
import type { Message, ToolIndicator } from './chat-types';

interface MessageListProps {
  messages: Message[];
  streamingText: string;
  activeTools: ToolIndicator[];
  isLoading: boolean;
  lastUserMsgId: string | null;
  copiedId: string | null;
  onCopyMessage: (msg: Message) => void;
  onRetryMessage: (msg: Message) => void;
  onOpenInCanvas?: (code: string, language: string) => void;
}

function formatTime(date: Date): string {
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

/** Extract display text from a message content string.
 *  If the content is a JSON array of Claude content blocks, extract and join
 *  the text blocks. Otherwise return as-is. */
export function extractDisplayText(content: string): string {
  if (!content.startsWith('[')) return content;
  try {
    const blocks = JSON.parse(content);
    if (!Array.isArray(blocks)) return content;
    const texts = blocks
      .filter((b: any) => b && b.type === 'text' && typeof b.text === 'string')
      .map((b: any) => b.text);
    return texts.length > 0 ? texts.join('') : content;
  } catch {
    return content;
  }
}

// ToolStatusStrip handles running/retrying/done/error states with countdown timers.

function MessageItem({
  msg,
  isLastUser,
  isLoading,
  copiedId,
  markdownComponents,
  onCopy,
  onRetry,
}: {
  msg: Message;
  isLastUser: boolean;
  isLoading: boolean;
  copiedId: string | null;
  markdownComponents: object;
  onCopy: (msg: Message) => void;
  onRetry: (msg: Message) => void;
}) {
  const canRetry = msg.role === 'user' && isLastUser && !isLoading;
  return (
    <div className={`message ${msg.role}${msg.isError ? ' error' : ''}`}>
      <div
        className="message-role"
        role="img"
        aria-label={msg.isError ? 'Error' : msg.role === 'user' ? 'You' : 'Assistant'}
      >
        {msg.isError ? '⚠️' : msg.role === 'user' ? '👤' : '🤖'}
      </div>
      <div className="message-body">
        <div className="message-content">
          {msg.toolIndicators && <ToolStatusStrip tools={msg.toolIndicators} />}
          <ReactMarkdown components={markdownComponents as any}>{extractDisplayText(msg.content)}</ReactMarkdown>
          <div className="message-actions">
            <button
              className="message-action-btn"
              title="Copy"
              aria-label={copiedId === msg.id ? 'Copied' : 'Copy message'}
              onClick={() => onCopy(msg)}
            >
              {copiedId === msg.id ? '✓' : '⎘'}
            </button>
            {canRetry && (
              <button
                className="message-action-btn"
                title="Edit and retry"
                aria-label="Edit and retry"
                onClick={() => onRetry(msg)}
              >
                ↺
              </button>
            )}
          </div>
        </div>
        <div className="message-meta">
          {msg.model && <span className="message-model">{msg.model}</span>}
          <span className="message-time">{formatTime(msg.timestamp)}</span>
        </div>
      </div>
    </div>
  );
}

export default function MessageList({
  messages,
  streamingText,
  activeTools,
  isLoading,
  lastUserMsgId,
  copiedId,
  onCopyMessage,
  onRetryMessage,
  onOpenInCanvas,
}: MessageListProps) {
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, streamingText, activeTools]);

  const markdownComponents = {
    code({ className, children, ...props }: any) {
      const match = /language-(\w+)/.exec(className || '');
      const codeString = String(children).replace(/\n$/, '');
      if (!match && !className) return <code className={className} {...props}>{children}</code>;
      return <CodeBlock code={codeString} language={match ? match[1] : ''} onOpenInCanvas={onOpenInCanvas} />;
    },
    pre({ children }: any) { return <>{children}</>; },
  };

  return (
    <div className="messages" role="log" aria-live="polite" aria-label="Chat messages" aria-relevant="additions">
      {messages.length === 0 && !streamingText && (
        <div className="welcome">
          <div className="welcome-icon">🤖</div>
          <h2>NexiBot</h2>
          <p className="welcome-subtitle">AI assistant with tools, memory, and voice</p>
          <div className="welcome-hints">
            <div className="welcome-hint-group">
              <span className="welcome-hint-label">Voice</span>
              <span className="welcome-hint-text">Start voice in the bar below, then say <em>"Hey Nexus"</em></span>
            </div>
            <div className="welcome-hint-group">
              <span className="welcome-hint-label">Push-to-talk</span>
              <span className="welcome-hint-text">Hold the 🎙️ button to speak directly</span>
            </div>
            <div className="welcome-hint-group">
              <span className="welcome-hint-label">Commands</span>
              <span className="welcome-hint-text">Type <code>/</code> to see available slash commands</span>
            </div>
            <div className="welcome-hint-group">
              <span className="welcome-hint-label">Models</span>
              <span className="welcome-hint-text">Switch with <code>/model opus</code>, <code>/model sonnet</code>, or <code>/model haiku</code></span>
            </div>
          </div>
        </div>
      )}

      {messages.map((msg) => (
        <MessageItem
          key={msg.id}
          msg={msg}
          isLastUser={msg.id === lastUserMsgId}
          isLoading={isLoading}
          copiedId={copiedId}
          markdownComponents={markdownComponents}
          onCopy={onCopyMessage}
          onRetry={onRetryMessage}
        />
      ))}

      {/* Streaming assistant message */}
      {(streamingText || activeTools.length > 0) && (
        <div className="message assistant streaming">
          <div className="message-role">🤖</div>
          <div className="message-body">
            <div className="message-content">
              {activeTools.length > 0 && <ToolStatusStrip tools={activeTools} />}
              {streamingText && <ReactMarkdown components={markdownComponents as any}>{streamingText}</ReactMarkdown>}
              {!streamingText && activeTools.length > 0 && activeTools.some(t => t.status === 'running' || t.status === 'retrying') && (
                <div className="typing-indicator"><span /><span /><span /></div>
              )}
            </div>
          </div>
        </div>
      )}

      {isLoading && !streamingText && activeTools.length === 0 && (
        <div className="message assistant">
          <div className="message-role">🤖</div>
          <div className="message-body">
            <div className="message-content">
              <div className="typing-indicator"><span /><span /><span /></div>
            </div>
          </div>
        </div>
      )}

      <div ref={messagesEndRef} />
    </div>
  );
}
