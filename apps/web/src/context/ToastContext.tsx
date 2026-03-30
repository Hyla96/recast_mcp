import React, {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
} from 'react';
import ReactDOM from 'react-dom';

export type ToastVariant = 'success' | 'error' | 'info';

export interface Toast {
  id: string;
  message: string;
  variant: ToastVariant;
  autoDismiss: boolean;
}

interface ToastContextValue {
  toasts: Toast[];
  addToast: (message: string, variant?: ToastVariant) => void;
  removeToast: (id: string) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

const MAX_VISIBLE = 3;
const AUTO_DISMISS_MS = 5000;

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const timerRefs = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  const removeToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
    const timer = timerRefs.current.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      timerRefs.current.delete(id);
    }
  }, []);

  const addToast = useCallback(
    (message: string, variant: ToastVariant = 'info') => {
      const id = Math.random().toString(36).slice(2);
      const autoDismiss = variant !== 'error';

      setToasts((prev) => {
        const next = [...prev, { id, message, variant, autoDismiss }];
        return next.slice(-MAX_VISIBLE);
      });

      if (autoDismiss) {
        const timer = setTimeout(() => removeToast(id), AUTO_DISMISS_MS);
        timerRefs.current.set(id, timer);
      }
    },
    [removeToast]
  );

  return (
    <ToastContext.Provider value={{ toasts, addToast, removeToast }}>
      {children}
      <ToastPortal toasts={toasts} onDismiss={removeToast} />
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (ctx === null) {
    throw new Error('useToast must be used within ToastProvider');
  }
  return ctx;
}

// ─── Toast Portal ─────────────────────────────────────────────────────────

function ToastPortal({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: string) => void;
}) {
  const container = document.getElementById('toast-root') ?? document.body;

  return ReactDOM.createPortal(
    <div
      aria-live="polite"
      aria-label="Notifications"
      className="fixed bottom-20 right-20 z-toast flex flex-col gap-8 pointer-events-none"
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>,
    container
  );
}

function ToastItem({
  toast,
  onDismiss,
}: {
  toast: Toast;
  onDismiss: (id: string) => void;
}) {
  const variantClasses: Record<ToastVariant, string> = {
    success: 'bg-secondary text-on-primary',
    error: 'bg-error text-on-primary',
    info: 'bg-surface-container-lowest text-text-primary',
  };

  return (
    <div
      role="status"
      className={`pointer-events-auto flex items-start gap-8 rounded-md px-16 py-12 shadow-float max-w-xs ${variantClasses[toast.variant]}`}
    >
      <span className="flex-1 text-sm">{toast.message}</span>
      <button
        type="button"
        onClick={() => onDismiss(toast.id)}
        aria-label="Dismiss notification"
        className="ml-8 shrink-0 opacity-70 hover:opacity-100 transition-opacity"
      >
        ×
      </button>
    </div>
  );
}
