import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings, PairingRequest } from '../SettingsContext';
import { notifyError } from '../../../shared/notify';

interface PairingSectionProps {
  channel: string;
  allowlistLabel?: string;
}

export function PairingSection({ channel, allowlistLabel }: PairingSectionProps) {
  const { pairingRequests, runtimeAllowlist, loadPairingData } = useSettings();
  const [processingCode, setProcessingCode] = useState<string | null>(null);
  const [removingItem, setRemovingItem] = useState<string | null>(null);

  const channelRequests = pairingRequests.filter(r => r.channel === channel);

  // Get the allowlist for this specific channel from runtime
  const runtimeItems = channel === 'telegram'
    ? runtimeAllowlist.telegram.map(String)
    : channel === 'whatsapp'
    ? runtimeAllowlist.whatsapp
    : (runtimeAllowlist.channels[channel] || []);

  return (
    <>
      {channelRequests.length > 0 && (
        <div style={{ margin: '8px 0' }}>
          <h4>Pending Pairing Requests</h4>
          {channelRequests.map((req: PairingRequest) => (
            <div key={req.code} className="mcp-server-card">
              <div className="mcp-server-header">
                <span className="mcp-server-name">{req.display_name || req.id}</span>
                <span className="mcp-server-command" style={{ fontFamily: 'monospace' }}>{req.code}</span>
                <span className="mcp-tool-count">{new Date(req.created_at).toLocaleString()}</span>
              </div>
              <div className="action-buttons">
                <button className="primary" disabled={processingCode === req.code} onClick={async () => {
                  setProcessingCode(req.code);
                  try {
                    await invoke('approve_pairing_code', { code: req.code });
                    loadPairingData();
                  } catch (error) {
                    notifyError('Pairing', `Failed to approve pairing: ${error}`);
                  } finally {
                    setProcessingCode(null);
                  }
                }}>{processingCode === req.code ? 'Approving…' : 'Approve'}</button>
                <button className="danger" disabled={processingCode === req.code} onClick={async () => {
                  setProcessingCode(req.code);
                  try {
                    await invoke('deny_pairing_code', { code: req.code });
                    loadPairingData();
                  } catch (error) {
                    notifyError('Pairing', `Failed to deny pairing: ${error}`);
                  } finally {
                    setProcessingCode(null);
                  }
                }}>{processingCode === req.code ? 'Denying…' : 'Deny'}</button>
              </div>
            </div>
          ))}
        </div>
      )}

      {runtimeItems.length > 0 && (
        <div style={{ margin: '8px 0' }}>
          <h4>Approved Senders</h4>
          {runtimeItems.map((item) => (
            <div key={item} className="mcp-server-card">
              <div className="mcp-server-header">
                <span className="mcp-server-name">{allowlistLabel ? `${allowlistLabel}: ${item}` : item}</span>
                <button className="mcp-remove-btn" disabled={removingItem === item} onClick={async () => {
                  setRemovingItem(item);
                  try {
                    await invoke('remove_from_allowlist', { channel, senderId: String(item) });
                    loadPairingData();
                  } catch (error) {
                    notifyError('Pairing', `Failed to remove from allowlist: ${error}`);
                  } finally {
                    setRemovingItem(null);
                  }
                }}>{removingItem === item ? 'Removing…' : 'Remove'}</button>
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
