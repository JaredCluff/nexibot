import { useState, ReactNode } from 'react';

interface CollapsibleSectionProps {
  title: string;
  defaultOpen?: boolean;
  children: ReactNode;
}

export function CollapsibleSection({ title, defaultOpen = false, children }: CollapsibleSectionProps) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div className="settings-group collapsible-section">
      <h3
        onClick={() => setOpen(!open)}
        style={{ cursor: 'pointer', userSelect: 'none', display: 'flex', alignItems: 'center', gap: '6px' }}
      >
        <span style={{ fontSize: '10px', transition: 'transform 0.15s', transform: open ? 'rotate(90deg)' : 'none' }}>
          {'\u25B6'}
        </span>
        {title}
      </h3>
      {open && children}
    </div>
  );
}
