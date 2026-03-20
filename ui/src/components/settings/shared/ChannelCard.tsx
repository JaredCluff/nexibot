import { ReactNode } from 'react';
import { InfoTip } from './InfoTip';
import { InfoGuide } from './InfoGuide';

interface ChannelCardProps {
  name: string;
  tooltip: string;
  guideTitle?: string;
  guideContent?: ReactNode;
  enabled: boolean;
  onToggle: (enabled: boolean) => void;
  children?: ReactNode;
}

export function ChannelCard({ name, tooltip, guideTitle, guideContent, enabled, onToggle, children }: ChannelCardProps) {
  return (
    <div className="settings-group">
      <h3>{name} <InfoTip text={tooltip} /></h3>
      {guideTitle && guideContent && (
        <InfoGuide title={guideTitle}>{guideContent}</InfoGuide>
      )}
      <div className="inline-toggle">
        <label className="toggle-label">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => onToggle(e.target.checked)}
          />
          Enable {name}
        </label>
      </div>
      {enabled && children}
    </div>
  );
}
