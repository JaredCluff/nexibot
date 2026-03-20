import { ReactNode } from 'react';

/** Inline info tooltip icon. Hover to see description. */
export function InfoTip({ text, children }: { text: string; children?: ReactNode }) {
  return (
    <span className="info-tip">
      <span className="info-tip-icon">i</span>
      <span className="info-tip-content">
        {text}
        {children}
      </span>
    </span>
  );
}
