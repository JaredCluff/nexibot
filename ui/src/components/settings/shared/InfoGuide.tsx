import { useState, ReactNode } from 'react';

/** Expandable how-to guide for integration setup. */
export function InfoGuide({ title, children }: { title: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="info-guide">
      <button className="info-guide-header" onClick={() => setOpen(!open)}>
        {open ? '\u25BC' : '\u25B6'} {title}
      </button>
      {open && <div className="info-guide-body">{children}</div>}
    </div>
  );
}
