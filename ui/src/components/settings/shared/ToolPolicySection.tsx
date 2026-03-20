import { useMemo } from 'react';
import { ChannelToolPolicy, useSettings } from '../SettingsContext';
import { CollapsibleSection } from './CollapsibleSection';
import { TagInput } from './TagInput';
import type { TagSuggestion } from './suggestions';
import { BUILTIN_TOOL_SUGGESTIONS } from './suggestions';
import { InfoTip } from './InfoTip';

interface ToolPolicySectionProps {
  policy: ChannelToolPolicy;
  onChange: (policy: ChannelToolPolicy) => void;
}

export function ToolPolicySection({ policy, onChange }: ToolPolicySectionProps) {
  const { mcpServers } = useSettings();

  const allToolSuggestions = useMemo<TagSuggestion[]>(() => {
    const mcpTools: TagSuggestion[] = mcpServers.flatMap((s) =>
      s.tools.map((t) => ({
        value: t.prefixed_name,
        description: t.description || `Tool from ${t.server_name}`,
        badge: 'elevated' as const,
      }))
    );
    return [...BUILTIN_TOOL_SUGGESTIONS, ...mcpTools];
  }, [mcpServers]);

  return (
    <CollapsibleSection title="Tool Policy">
      <label className="field">
        <span>Denied Tools <InfoTip text="Tool names denied on this channel. Users on this channel cannot invoke these tools." /></span>
      </label>
      <TagInput
        tags={policy.denied_tools}
        onChange={(tags) => onChange({ ...policy, denied_tools: tags })}
        placeholder="e.g., nexibot_execute"
        suggestions={allToolSuggestions}
      />

      <label className="field">
        <span>Allowed Tools (Override) <InfoTip text="Tools explicitly allowed even if listed in denied tools. Use this to grant specific tool access while keeping the rest denied." /></span>
      </label>
      <TagInput
        tags={policy.allowed_tools}
        onChange={(tags) => onChange({ ...policy, allowed_tools: tags })}
        placeholder="e.g., nexibot_fetch"
        suggestions={allToolSuggestions}
      />

      <div className="inline-toggle">
        <label className="toggle-label">
          <input
            type="checkbox"
            checked={policy.admin_bypass}
            onChange={(e) => onChange({ ...policy, admin_bypass: e.target.checked })}
          />
          Admin Bypass <InfoTip text="When enabled, admin users bypass the denied tools list entirely." />
        </label>
      </div>
    </CollapsibleSection>
  );
}
