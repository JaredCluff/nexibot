import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { notifyError } from '../shared/notify';
import './YoloApprovalBanner.css';

interface YoloRequest {
  id: string;
  requested_at_ms: number;
  duration_secs: number | null;
  reason: string | null;
}

interface YoloStatus {
  active: boolean;
  approved_at_ms: number | null;
  expires_at_ms: number | null;
  remaining_secs: number | null;
  pending_request: YoloRequest | null;
}

interface YoloCmdResult {
  ok: boolean;
  message: string;
  status: YoloStatus | null;
}

function formatDuration(secs: number | null): string {
  if (secs === null) return 'no time limit';
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  return `${Math.round(secs / 3600)}h`;
}

function YoloApprovalBanner() {
  const [pendingRequest, setPendingRequest] = useState<YoloRequest | null>(null);
  const [activeStatus, setActiveStatus] = useState<YoloStatus | null>(null);
  const [approving, setApproving] = useState(false);

  const syncStatus = useCallback(async () => {
    try {
      const status = await invoke<YoloStatus>('get_yolo_status');
      setPendingRequest(status.pending_request ?? null);
      setActiveStatus(status.active ? status : null);
    } catch {
      // Keep current UI state if a status refresh fails.
    }
  }, []);

  // Keep UI state synced even when requests/approvals happen out-of-band
  // (e.g. phone approval from Telegram or headless tool-call path).
  useEffect(() => {
    syncStatus();
    const interval = setInterval(() => {
      void syncStatus();
    }, 4000);
    return () => clearInterval(interval);
  }, [syncStatus]);

  // Countdown for active yolo mode. Depend only on activeStatus?.active so the
  // interval is not cleared and recreated on every 1-second state update.
  const isTimedSession = activeStatus?.active && activeStatus.remaining_secs !== null;
  useEffect(() => {
    if (!isTimedSession) return;
    const interval = setInterval(() => {
      void syncStatus();
    }, 1000);
    return () => clearInterval(interval);
  }, [isTimedSession, syncStatus]);

  const handleApprove = useCallback(async () => {
    if (!pendingRequest) return;
    setApproving(true);
    try {
      const result = await invoke<YoloCmdResult>('approve_yolo_mode', {
        requestId: pendingRequest.id,
      });
      if (result.ok && result.status) {
        setPendingRequest(null);
        setActiveStatus(result.status);
      } else {
        notifyError('Yolo Approval Failed', result.message);
      }
    } catch (e) {
      notifyError('Yolo Approval Error', String(e));
    } finally {
      setApproving(false);
    }
  }, [pendingRequest]);

  const handleDeny = useCallback(async () => {
    try {
      await invoke('revoke_yolo_mode');
    } catch {/* ignore */}
    setPendingRequest(null);
  }, []);

  const handleRevoke = useCallback(async () => {
    try {
      await invoke('revoke_yolo_mode');
    } catch {/* ignore */}
    setActiveStatus(null);
  }, []);

  // Listen for backend events
  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<YoloRequest>('yolo:request-pending', (event) => {
        setPendingRequest(event.payload);
      })
    );

    unlisteners.push(
      listen<YoloStatus>('yolo:approved', (event) => {
        setPendingRequest(null);
        setActiveStatus(event.payload);
      })
    );

    unlisteners.push(
      listen('yolo:revoked', () => {
        setPendingRequest(null);
        setActiveStatus(null);
      })
    );

    unlisteners.push(
      listen('yolo:expired', () => {
        setActiveStatus(null);
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // Active yolo mode indicator
  if (activeStatus?.active) {
    const remaining = activeStatus.remaining_secs;
    const label = remaining !== null ? `${remaining}s remaining` : 'active (no limit)';
    return (
      <div className="yolo-active-banner">
        <span className="yolo-active-icon">⚡</span>
        <span className="yolo-active-label">Yolo mode {label}</span>
        <button className="yolo-revoke-btn" onClick={handleRevoke}>
          Revoke
        </button>
      </div>
    );
  }

  // Pending approval request
  if (!pendingRequest) return null;

  const duration = formatDuration(pendingRequest.duration_secs);
  const requestedAt = new Date(pendingRequest.requested_at_ms).toLocaleTimeString();

  return (
    <div className="yolo-approval-overlay">
      <div className="yolo-approval-card">
        <div className="yolo-approval-header">
          <span className="yolo-approval-icon">⚡</span>
          <h3 className="yolo-approval-title">Yolo Mode Request</h3>
        </div>

        <p className="yolo-approval-body">
          The model is requesting elevated access ({duration}) at {requestedAt}.
        </p>

        {pendingRequest.reason && (
          <div className="yolo-approval-reason">
            <strong>Reason:</strong> {pendingRequest.reason}
          </div>
        )}

        <p className="yolo-approval-warning">
          Approving allows the model to modify config files and take privileged
          actions. Only approve if you initiated this request.
        </p>

        <div className="yolo-approval-actions">
          <button
            className="yolo-deny-btn"
            onClick={handleDeny}
            disabled={approving}
          >
            Deny
          </button>
          <button
            className="yolo-approve-btn"
            onClick={handleApprove}
            disabled={approving}
          >
            {approving ? 'Approving…' : 'Approve'}
          </button>
        </div>
      </div>
    </div>
  );
}

export default YoloApprovalBanner;
