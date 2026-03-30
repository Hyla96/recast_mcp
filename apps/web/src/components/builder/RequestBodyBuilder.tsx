/**
 * RequestBodyBuilder — inline request-body editor for POST/PUT/PATCH steps.
 *
 * Renders a code-editor-style textarea with side-by-side line numbers,
 * debounced JSON validation (400 ms), template-variable detection, collision
 * warnings against existing path/query params, and a 'Load example body'
 * dropdown with three pre-defined patterns.
 *
 * Design choices:
 *  - Uncontrolled textarea: content is accessed via `ref`; the Zustand store
 *    is updated synchronously on every change so that template-var detection
 *    (via `useMemo` on `requestBody`) stays live.
 *  - Line numbers are synced to the textarea scroll position via
 *    `requestAnimationFrame` — never with forced synchronous layout.
 *  - Template variables derived from the store's `requestBody` via `useMemo`;
 *    editable label / required metadata kept in local React state.
 *  - Replace-confirmation uses an inline `pendingExample` state rather than
 *    `window.confirm()`.
 */

import { useRef, useState, useMemo, useEffect, useCallback } from 'react';
import type { ChangeEvent } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import { extractTemplateVars } from '@/lib/requestBodyParser';
import { parseJsonWithLineNumbers } from '@/lib/jsonValidator';

// ── Constants ─────────────────────────────────────────────────────────────────

const LINE_HEIGHT_REM = 1.5; // matches leading-6 / text-sm in Tailwind
const MIN_LINES = 8;
const MAX_LINES = 24;
const VALIDATION_DEBOUNCE_MS = 400;
const IDLE_THRESHOLD_BYTES = 50_000;

const EXAMPLE_BODIES = [
  {
    label: 'Simple object',
    body: '{\n  "key": "{{key}}",\n  "value": {{value}}\n}',
  },
  {
    label: 'User creation',
    body: '{\n  "name": "{{name}}",\n  "email": "{{email}}",\n  "age": {{age}}\n}',
  },
  {
    label: 'Nested payload',
    body: [
      '{',
      '  "data": {',
      '    "id": "{{id}}",',
      '    "attributes": {',
      '      "title": "{{title}}",',
      '      "count": {{count}}',
      '    }',
      '  }',
      '}',
    ].join('\n'),
  },
] as const;

// ── Local types ───────────────────────────────────────────────────────────────

interface VarMeta {
  label: string;
  required: boolean;
}

type ValidationState =
  | { valid: true }
  | { valid: false; error: string; line: number | null }
  | null;

// ── Component ─────────────────────────────────────────────────────────────────

interface RequestBodyBuilderProps {
  /**
   * Initial textarea content (restored from a prior method-switch).
   * Pass `''` when the editor should start blank.
   */
  initialContent: string;
  /**
   * Called on every content change so the parent can cache the raw body
   * string for restoration if the HTTP method is switched away and back.
   */
  onContentChange: (content: string) => void;
}

export function RequestBodyBuilder({
  initialContent,
  onContentChange,
}: RequestBodyBuilderProps) {
  // ── Store ──────────────────────────────────────────────────────────────────

  const setRequestBody = useBuilderStore((s) => s.setRequestBody);
  const requestBody = useBuilderStore((s) => s.requestBodySlice.requestBody);

  const pathParams = useBuilderStore((s) => s.urlSlice.pathParams);
  const queryParams = useBuilderStore((s) => s.urlSlice.queryParams);

  // Names already used as path / query params — for collision detection.
  const existingParamNames = useMemo(
    () =>
      new Set<string>([
        ...pathParams.map((p) => p.name),
        ...queryParams.map((q) => q.key),
      ]),
    [pathParams, queryParams]
  );

  // ── Refs ──────────────────────────────────────────────────────────────────

  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const lineNumbersRef = useRef<HTMLDivElement | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rafRef = useRef<number | null>(null);

  // ── Local state ───────────────────────────────────────────────────────────

  const [validationState, setValidationState] = useState<ValidationState>(null);
  const [showExampleDropdown, setShowExampleDropdown] = useState(false);
  // Content of a chosen example that is awaiting replace-confirmation.
  const [pendingExample, setPendingExample] = useState<string | null>(null);
  // Per-variable editable metadata (label and required flag).
  const [varMeta, setVarMeta] = useState<Map<string, VarMeta>>(new Map());

  // ── Template variables (derived from store, not stored in Zustand) ────────

  const templateVars = useMemo(
    () => (requestBody !== null ? extractTemplateVars(requestBody) : []),
    [requestBody]
  );

  // Merge varMeta whenever the derived variable list changes:
  // preserve existing user edits, add defaults for newly detected vars.
  useEffect(() => {
    setVarMeta((prev) => {
      const next = new Map<string, VarMeta>();
      for (const v of templateVars) {
        const existing = prev.get(v.name);
        next.set(v.name, existing ?? { label: v.name, required: true });
      }
      return next;
    });
  }, [templateVars]);

  // ── Helpers ───────────────────────────────────────────────────────────────

  const updateLineNumbers = useCallback(() => {
    const ta = textareaRef.current;
    const ln = lineNumbersRef.current;
    if (ta === null || ln === null) return;

    const lines = ta.value.split('\n');
    let html = '';
    for (let i = 1; i <= lines.length; i++) {
      html += `<div>${i}</div>`;
    }
    ln.innerHTML = html;
    ln.scrollTop = ta.scrollTop;
  }, []);

  const scheduleLineNumberUpdate = useCallback(() => {
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(() => {
      updateLineNumbers();
      rafRef.current = null;
    });
  }, [updateLineNumbers]);

  const runValidation = useCallback((content: string) => {
    if (content.trim() === '') {
      setValidationState(null);
      return;
    }
    const result = parseJsonWithLineNumbers(content);
    setValidationState(
      result.ok
        ? { valid: true }
        : { valid: false, error: result.error, line: result.line }
    );
  }, []);

  // ── Initialization ────────────────────────────────────────────────────────

  useEffect(() => {
    const ta = textareaRef.current;
    if (ta === null) return;

    if (initialContent !== '') {
      ta.value = initialContent;
      setRequestBody(initialContent);
      onContentChange(initialContent);
      runValidation(initialContent);
    } else {
      // Ensure the store is initialized to '' (not null) for body-carrying
      // methods, so template-var detection is active immediately.
      setRequestBody('');
    }

    updateLineNumbers();
    // Mount-only — intentionally no deps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Cleanup pending timers and rAF on unmount.
  useEffect(() => {
    return () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
      if (debounceRef.current !== null) clearTimeout(debounceRef.current);
    };
  }, []);

  // ── Handlers ──────────────────────────────────────────────────────────────

  const handleChange = useCallback(
    (e: ChangeEvent<HTMLTextAreaElement>) => {
      const content = e.target.value;

      // Synchronous store update so template vars update in real time.
      setRequestBody(content);
      onContentChange(content);

      scheduleLineNumberUpdate();

      // Debounced JSON validation.
      if (debounceRef.current !== null) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        debounceRef.current = null;

        if (content.length > IDLE_THRESHOLD_BYTES) {
          // Defer expensive validation for large pastes.
          if (typeof requestIdleCallback !== 'undefined') {
            requestIdleCallback(() => runValidation(content));
          } else {
            setTimeout(() => runValidation(content), 0);
          }
        } else {
          runValidation(content);
        }
      }, VALIDATION_DEBOUNCE_MS);
    },
    [setRequestBody, onContentChange, scheduleLineNumberUpdate, runValidation]
  );

  const handleScroll = useCallback(() => {
    scheduleLineNumberUpdate();
  }, [scheduleLineNumberUpdate]);

  const applyExample = useCallback(
    (body: string) => {
      const ta = textareaRef.current;
      if (ta === null) return;

      ta.value = body;
      setRequestBody(body);
      onContentChange(body);
      scheduleLineNumberUpdate();
      runValidation(body);
      setShowExampleDropdown(false);
      setPendingExample(null);
    },
    [setRequestBody, onContentChange, scheduleLineNumberUpdate, runValidation]
  );

  const handleExampleSelect = useCallback(
    (body: string) => {
      const currentContent = textareaRef.current?.value ?? '';
      if (currentContent.trim() !== '') {
        // Non-empty editor: request confirmation before overwriting.
        setPendingExample(body);
        setShowExampleDropdown(false);
      } else {
        applyExample(body);
      }
    },
    [applyExample]
  );

  // ── Render ────────────────────────────────────────────────────────────────

  const editorStyle = {
    lineHeight: `${LINE_HEIGHT_REM}rem`,
    minHeight: `${MIN_LINES * LINE_HEIGHT_REM}rem`,
    maxHeight: `${MAX_LINES * LINE_HEIGHT_REM}rem`,
  } as const;

  return (
    <div data-testid="request-body-builder" className="space-y-16">
      {/* ── Header ── */}
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-text-primary">Request body</h3>

        {/* Load example body dropdown */}
        <div className="relative">
          <button
            type="button"
            data-testid="load-example-body-btn"
            onClick={() => setShowExampleDropdown((v) => !v)}
            className="text-xs text-brand-500 hover:underline focus:outline-none focus:ring-1 focus:ring-brand-500 rounded-sm"
          >
            Load example body
          </button>

          {showExampleDropdown && (
            <div
              role="menu"
              className="absolute right-0 top-full z-10 mt-4 min-w-48 rounded-md border border-border-default bg-surface-container-lowest py-4 shadow-float"
            >
              {EXAMPLE_BODIES.map((ex) => (
                <button
                  key={ex.label}
                  type="button"
                  role="menuitem"
                  onClick={() => handleExampleSelect(ex.body)}
                  className="block w-full px-12 py-8 text-left text-sm text-text-primary hover:bg-surface-container focus:bg-surface-container focus:outline-none"
                >
                  {ex.label}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* ── Inline replace-confirmation ── */}
      {pendingExample !== null && (
        <div
          role="alert"
          className="flex items-center gap-12 rounded-md border border-amber-200 bg-amber-50 px-12 py-10 dark:border-amber-800 dark:bg-amber-950"
        >
          <span className="flex-1 text-sm text-amber-800 dark:text-amber-200">
            Replace current content?
          </span>
          <button
            type="button"
            onClick={() => applyExample(pendingExample)}
            className="text-sm font-medium text-amber-700 hover:underline dark:text-amber-300"
          >
            Replace
          </button>
          <button
            type="button"
            onClick={() => setPendingExample(null)}
            className="text-sm text-text-secondary hover:underline"
          >
            Cancel
          </button>
        </div>
      )}

      {/* ── Code editor: line numbers + textarea ── */}
      <div className="overflow-hidden rounded-md border border-border-default">
        <div className="flex">
          {/* Line numbers panel */}
          <div
            ref={lineNumbersRef}
            aria-hidden="true"
            className="select-none overflow-hidden bg-surface-container px-8 py-8 text-right font-mono text-xs text-text-secondary"
            style={editorStyle}
          />

          {/* Textarea — uncontrolled, accessed via ref */}
          <textarea
            ref={textareaRef}
            data-testid="request-body-editor"
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            onChange={handleChange}
            onScroll={handleScroll}
            placeholder={'{\n  "key": "value"\n}'}
            className="flex-1 resize-none overflow-y-auto bg-surface-container-lowest py-8 pl-12 pr-12 font-mono text-sm text-text-primary focus:outline-none"
            style={editorStyle}
          />
        </div>
      </div>

      {/* ── Validation badge ── */}
      {validationState !== null && (
        <div>
          {validationState.valid ? (
            <span className="inline-flex items-center gap-4 rounded-sm bg-secondary-container px-8 py-2 text-xs font-medium text-secondary">
              {/* Checkmark icon */}
              <svg
                className="h-12 w-12 shrink-0"
                viewBox="0 0 12 12"
                fill="none"
                aria-hidden="true"
              >
                <path
                  d="M2 6l3 3 5-5"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
              Valid JSON
            </span>
          ) : (
            <p className="text-xs text-error-DEFAULT" role="alert">
              {validationState.line !== null
                ? `Line ${validationState.line}: ${validationState.error}`
                : validationState.error}
            </p>
          )}
        </div>
      )}

      {/* ── Template variable list ── */}
      {templateVars.length > 0 && (
        <div className="space-y-12">
          <h4 className="text-xs font-medium uppercase tracking-wide text-text-secondary">
            Input parameters
          </h4>

          <div className="space-y-8">
            {templateVars.map((v) => {
              const meta = varMeta.get(v.name) ?? { label: v.name, required: true };
              const hasCollision = existingParamNames.has(v.name);

              return (
                <div
                  key={v.name}
                  className="space-y-8 rounded-md bg-surface-container p-12"
                >
                  {/* Top row: name, type badge, required/optional toggle */}
                  <div className="flex items-center gap-12">
                    <code className="shrink-0 font-mono text-xs text-text-primary">
                      {`{{${v.name}}}`}
                    </code>

                    <span className="shrink-0 rounded-sm bg-surface-variant px-6 py-1 text-xs text-text-secondary">
                      {v.type}
                    </span>

                    <button
                      type="button"
                      onClick={() => {
                        setVarMeta((prev) => {
                          const next = new Map(prev);
                          const current = next.get(v.name) ?? {
                            label: v.name,
                            required: true,
                          };
                          next.set(v.name, {
                            ...current,
                            required: !current.required,
                          });
                          return next;
                        });
                      }}
                      aria-pressed={meta.required}
                      className={`ml-auto shrink-0 rounded-sm px-8 py-2 text-xs font-medium transition-colors focus:outline-none focus:ring-1 focus:ring-brand-500 ${
                        meta.required
                          ? 'bg-brand-100 text-brand-700 dark:bg-brand-900 dark:text-brand-300'
                          : 'bg-surface-variant text-text-secondary'
                      }`}
                    >
                      {meta.required ? 'Required' : 'Optional'}
                    </button>
                  </div>

                  {/* Label input row */}
                  <div className="flex items-center gap-8">
                    <label className="shrink-0 text-xs text-text-secondary">
                      Label:
                    </label>
                    <input
                      type="text"
                      value={meta.label}
                      onChange={(e) => {
                        const newLabel = e.target.value;
                        setVarMeta((prev) => {
                          const next = new Map(prev);
                          const current = next.get(v.name) ?? {
                            label: v.name,
                            required: true,
                          };
                          next.set(v.name, { ...current, label: newLabel });
                          return next;
                        });
                      }}
                      className="flex-1 rounded border border-border-default bg-surface-container-lowest px-8 py-4 text-xs text-text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
                      aria-label={`Claude-facing label for ${v.name}`}
                    />
                  </div>

                  {/* Collision warning */}
                  {hasCollision && (
                    <p
                      role="alert"
                      className="text-xs text-amber-700 dark:text-amber-300"
                    >
                      Parameter name already exists as a path/query parameter
                    </p>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
