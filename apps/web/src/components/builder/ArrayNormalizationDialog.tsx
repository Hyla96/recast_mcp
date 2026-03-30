/**
 * ArrayNormalizationDialog — confirmation shown when the user clicks a
 * renderer value whose JSONPath contains a numeric array index.
 *
 * Lets the user choose:
 *   - "Select for all items"   -> normalised path ($.items[*].price)
 *   - "Select first item only" -> original indexed path ($.items[0].price)
 *
 * Uses the native <dialog> element with showModal() for browser-native
 * focus trapping. On any close action, focus is restored to the element
 * that triggered the click in the DocumentRenderer.
 */

import { useEffect, useRef } from 'react';
import { getArrayContexts, type ArrayContext } from '@/lib/jsonPathUtils';

// ── Types ─────────────────────────────────────────────────────────────────────

export interface ArrayNormalizationDialogProps {
  /** The original JSONPath that was clicked (contains at least one [N]). */
  jsonPath: string;
  /** The full API response data — used to compute array counts and previews. */
  responseData: unknown;
  /** Called when the user chooses "Select for all items". */
  onNormalize: () => void;
  /** Called when the user chooses "Select first item only". */
  onKeepOriginal: () => void;
  /** Called when the dialog is dismissed without selection (Escape / backdrop). */
  onClose: () => void;
  /** Element to restore focus to on any close action. May be null. */
  triggerEl: HTMLElement | null;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Short display string for a raw preview value (max 32 chars). */
function formatPreviewValue(v: unknown): string {
  if (v === null || v === undefined) return '\u2014'; // em dash
  if (typeof v === 'boolean') return v ? 'Yes' : 'No';
  if (typeof v === 'number' || typeof v === 'string') {
    const s = String(v);
    return s.length > 32 ? s.slice(0, 29) + '...' : s;
  }
  try {
    const s = JSON.stringify(v);
    return s.length > 32 ? s.slice(0, 29) + '...' : s;
  } catch {
    return '[object]';
  }
}

// ── Component ─────────────────────────────────────────────────────────────────

export function ArrayNormalizationDialog({
  jsonPath,
  responseData,
  onNormalize,
  onKeepOriginal,
  onClose,
  triggerEl,
}: ArrayNormalizationDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  // Compute array contexts from the JSONPath and response data.
  const contexts: ArrayContext[] = getArrayContexts(jsonPath, responseData);
  const lastCtx: ArrayContext | undefined = contexts[contexts.length - 1];
  const hasMixedTypes = lastCtx?.hasMixedTypes ?? false;
  const previewValues = lastCtx?.previewValues ?? [];

  // Open the dialog on mount.
  useEffect(() => {
    dialogRef.current?.showModal();
  }, []);

  // Register the cancel (Escape) handler, refreshed when callbacks change.
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) return;

    const handleCancel = (e: Event) => {
      // Prevent the browser from auto-closing; we do it manually so focus
      // is restored before the close animation finishes.
      e.preventDefault();
      dialog.close();
      triggerEl?.focus();
      onClose();
    };

    dialog.addEventListener('cancel', handleCancel);
    return () => {
      dialog.removeEventListener('cancel', handleCancel);
    };
  }, [onClose, triggerEl]);

  // ── Internal action handlers ─────────────────────────────────────────────

  const handleNormalize = () => {
    dialogRef.current?.close();
    triggerEl?.focus();
    onNormalize();
  };

  const handleKeepOriginal = () => {
    dialogRef.current?.close();
    triggerEl?.focus();
    onKeepOriginal();
  };

  // Close when the user clicks the backdrop (outside the dialog box).
  const handleDialogClick = (e: React.MouseEvent<HTMLDialogElement>) => {
    const rect = dialogRef.current?.getBoundingClientRect();
    if (rect === undefined) return;
    const outside =
      e.clientX < rect.left ||
      e.clientX > rect.right ||
      e.clientY < rect.top ||
      e.clientY > rect.bottom;
    if (outside) {
      dialogRef.current?.close();
      triggerEl?.focus();
      onClose();
    }
  };

  // ── Render ───────────────────────────────────────────────────────────────

  return (
    <dialog
      ref={dialogRef}
      data-testid="array-normalization-dialog"
      onClick={handleDialogClick}
      className="m-auto rounded-lg border border-border-subtle bg-surface-container p-0 shadow-xl max-w-md w-full backdrop:bg-black/40"
    >
      <div className="p-24 space-y-16">
        {/* Heading */}
        <h2 className="text-base font-semibold text-text-primary">
          This value is inside a list
        </h2>

        {/* Array context explanation */}
        <div className="space-y-6">
          {contexts.map((ctx, i) => (
            <p key={i} className="text-sm text-text-secondary">
              {i === 0 ? 'This field lives in the ' : 'which is nested inside '}
              <span className="font-medium text-text-primary">{ctx.arrayName}</span>
              {' list '}
              <span className="text-text-secondary">({ctx.count} {ctx.count === 1 ? 'item' : 'items'})</span>
              {i < contexts.length - 1 ? ',' : '.'}
            </p>
          ))}
        </div>

        {/* Mixed-type warning */}
        {hasMixedTypes && (
          <div className="rounded-md bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800 px-12 py-8">
            <p className="text-xs text-amber-800 dark:text-amber-200">
              Note: this array contains mixed types
            </p>
          </div>
        )}

        {/* Preview values from the innermost array */}
        {previewValues.length > 0 && (
          <div>
            <p className="mb-8 text-xs font-medium uppercase tracking-wider text-text-secondary">
              Sample values
            </p>
            <ul className="divide-y divide-border-subtle rounded-md border border-border-subtle overflow-hidden">
              {previewValues.map((val, i) => (
                <li
                  key={i}
                  className="flex items-center gap-12 px-12 py-6 bg-surface-container-lowest"
                >
                  <span className="shrink-0 text-xs text-text-secondary font-mono tabular-nums w-16">
                    #{i + 1}
                  </span>
                  <span className="font-mono text-xs text-text-primary truncate">
                    {formatPreviewValue(val)}
                  </span>
                </li>
              ))}
            </ul>
          </div>
        )}

        {/* Action buttons */}
        <div className="flex flex-col gap-8 pt-8 border-t border-border-subtle">
          <button
            type="button"
            data-testid="normalize-array-confirm"
            onClick={handleNormalize}
            className="w-full rounded-md bg-brand-500 px-16 py-10 text-sm font-medium text-primary-on hover:bg-brand-600 focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-2 transition-colors"
          >
            Select for all items
          </button>
          <button
            type="button"
            data-testid="normalize-array-decline"
            onClick={handleKeepOriginal}
            className="w-full rounded-md border border-border-subtle px-16 py-10 text-sm font-medium text-text-primary hover:bg-surface-container-high focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-2 transition-colors"
          >
            Select first item only
          </button>
        </div>
      </div>
    </dialog>
  );
}
