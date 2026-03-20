import { invoke } from '@tauri-apps/api/core';
import { notifyError } from '../shared/notify';

interface GatedShellStatus {
  enabled: boolean;
  debug_mode: boolean;
  record_sessions: boolean;
  secret_count: number;
  active_sessions: number;
}

interface Props {
  status: GatedShellStatus | null;
  onStatusChange: (status: GatedShellStatus) => void;
}

export default function ShellViewerToolbar({ status, onStatusChange }: Props) {
  const handleToggleEnabled = async () => {
    try {
      const newStatus: GatedShellStatus = await invoke('set_gated_shell_enabled', {
        enabled: !status?.enabled,
      });
      onStatusChange(newStatus);
    } catch (e) {
      notifyError('NexiGate', `Failed to toggle shell gate: ${e}`);
    }
  };

  const handleToggleDebug = async (e: React.ChangeEvent<HTMLInputElement>) => {
    try {
      await invoke('set_gated_shell_debug', { debugMode: e.target.checked });
      const newStatus: GatedShellStatus = await invoke('get_gated_shell_status');
      onStatusChange(newStatus);
    } catch (e) {
      notifyError('NexiGate', `Failed to toggle debug mode: ${e}`);
    }
  };

  const handleToggleRecord = async (e: React.ChangeEvent<HTMLInputElement>) => {
    try {
      await invoke('set_gated_shell_record', { enabled: e.target.checked });
      const newStatus: GatedShellStatus = await invoke('get_gated_shell_status');
      onStatusChange(newStatus);
    } catch (e) {
      notifyError('NexiGate', `Failed to toggle session recording: ${e}`);
    }
  };

  return (
    <div className="shell-viewer-toolbar">
      <h2>NexiGate Shell Viewer</h2>

      {status?.enabled ? (
        <div className="shell-live-badge">
          <div className="shell-live-dot" />
          Live
        </div>
      ) : (
        <div className="shell-gate-disabled">Gate OFF</div>
      )}

      <label className="shell-toolbar-toggle" onClick={handleToggleEnabled}>
        <input
          type="checkbox"
          checked={status?.enabled ?? false}
          onChange={() => {}}
          readOnly
        />
        Enabled
      </label>

      <label className="shell-toolbar-toggle">
        <input
          type="checkbox"
          checked={status?.debug_mode ?? false}
          onChange={handleToggleDebug}
        />
        Debug
      </label>

      <label className="shell-toolbar-toggle">
        <input
          type="checkbox"
          checked={status?.record_sessions ?? false}
          onChange={handleToggleRecord}
        />
        Record
      </label>

      <div className="shell-toolbar-spacer" />

      <div className="shell-toolbar-status">
        {status
          ? `${status.active_sessions} session${status.active_sessions !== 1 ? 's' : ''} · ${status.secret_count} secret${status.secret_count !== 1 ? 's' : ''} masked`
          : 'Loading…'}
      </div>
    </div>
  );
}
