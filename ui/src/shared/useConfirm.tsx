import { useState, useCallback, ReactNode } from 'react';

interface ConfirmOptions {
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
}

interface ConfirmState {
  message: string;
  opts: ConfirmOptions;
  resolve: (value: boolean) => void;
}

/**
 * Hook that provides a non-blocking confirm dialog as a React portal.
 *
 * Usage:
 *   const { confirm, modal } = useConfirm();
 *   // In JSX: {modal}
 *   // In handler: if (!await confirm('Delete?', { danger: true })) return;
 */
export function useConfirm() {
  const [state, setState] = useState<ConfirmState | null>(null);

  const confirm = useCallback((message: string, opts: ConfirmOptions = {}): Promise<boolean> => {
    return new Promise((resolve) => {
      setState({ message, opts, resolve });
    });
  }, []);

  const handleConfirm = () => {
    state?.resolve(true);
    setState(null);
  };

  const handleCancel = () => {
    state?.resolve(false);
    setState(null);
  };

  const modal: ReactNode = state ? (
    <div className="confirm-overlay" onClick={handleCancel}>
      <div className="confirm-dialog" onClick={(e) => e.stopPropagation()}>
        <p className="confirm-message">{state.message}</p>
        <div className="confirm-actions">
          <button className="confirm-btn-cancel" onClick={handleCancel}>
            {state.opts.cancelLabel ?? 'Cancel'}
          </button>
          <button
            className={state.opts.danger !== false ? 'confirm-btn-danger' : 'confirm-btn-primary'}
            onClick={handleConfirm}
          >
            {state.opts.confirmLabel ?? 'Confirm'}
          </button>
        </div>
      </div>
    </div>
  ) : null;

  return { confirm, modal };
}
