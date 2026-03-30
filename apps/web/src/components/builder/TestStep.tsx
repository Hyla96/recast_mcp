/**
 * TestStep — step 3 of the builder flow.
 *
 * Sends a proxied test call to the configured API endpoint and renders
 * the response via DocumentRenderer. Manages loading, cancellation,
 * and all error states (4xx, 5xx, timeout, network error).
 *
 * The sample JSON escape hatch (TASK-008) adds textarea + JSON validation
 * when the user activates sample mode via the links rendered here.
 */

import { useState, useRef, useEffect, useCallback } from 'react';
import { useMutation } from '@tanstack/react-query';
import { useBuilderStore } from '@stores/builderStore';
import { useFetchWithAuth } from '@/lib/fetchWithAuth';
import { DocumentRenderer } from '@components/builder/DocumentRenderer';

// ── Types ──────────────────────────────────────────────────────────────────────

/**
 * Discriminated union returned by the test call mutation.
 * `'cancelled'` is an internal sentinel — the UI treats it as idle.
 */
export type TestCallResult =
  | { outcome: 'success'; statusCode: number; data: unknown }
  | { outcome: '4xx'; statusCode: number; statusText: string; data: unknown }
  | { outcome: '5xx'; statusCode: number; statusText: string }
  | { outcome: 'timeout' }
  | { outcome: 'network-error'; host: string }
  | { outcome: 'cancelled' };

interface TestCallPayload {
  url: string;
  method: string;
  pathParams: Record<string, string>;
  queryParams: Record<string, string>;
  auth: {
    type: 'none' | 'bearer' | 'api-key' | 'basic';
    bearerToken?: string | undefined;
    apiKeyName?: string | undefined;
    apiKeyValue?: string | undefined;
    apiKeyPlacement?: 'header' | 'query' | undefined;
    basicUsername?: string | undefined;
    basicPassword?: string | undefined;
  };
  body?: string | undefined;
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/** Status indicator icon rendered as a simple inline SVG. */
function StatusIcon({ ok }: { ok: boolean }) {
  if (ok) {
    return (
      <svg
        aria-hidden="true"
        className="h-16 w-16 shrink-0"
        viewBox="0 0 16 16"
        fill="none"
      >
        <circle cx="8" cy="8" r="7" className="fill-emerald-100 dark:fill-emerald-900" />
        <path
          d="M5 8l2 2 4-4"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-emerald-600 dark:text-emerald-400"
        />
      </svg>
    );
  }
  return (
    <svg
      aria-hidden="true"
      className="h-16 w-16 shrink-0"
      viewBox="0 0 16 16"
      fill="none"
    >
      <circle cx="8" cy="8" r="7" className="fill-red-100 dark:fill-red-900" />
      <path
        d="M5.5 5.5l5 5M10.5 5.5l-5 5"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        className="text-red-600 dark:text-red-400"
      />
    </svg>
  );
}

/** Spinner used while the test call is in-flight. */
function Spinner() {
  return (
    <svg
      aria-hidden="true"
      className="h-16 w-16 animate-spin text-brand-500"
      viewBox="0 0 24 24"
      fill="none"
    >
      <circle
        cx="12"
        cy="12"
        r="10"
        stroke="currentColor"
        strokeWidth="4"
        className="opacity-25"
      />
      <path
        d="M4 12a8 8 0 018-8"
        stroke="currentColor"
        strokeWidth="4"
        strokeLinecap="round"
        className="opacity-75"
      />
    </svg>
  );
}

// ── Component ──────────────────────────────────────────────────────────────────

export function TestStep({
  onContinue,
  onBack,
}: {
  onContinue: () => void;
  onBack: () => void;
}) {
  // ── Store state ─────────────────────────────────────────────────────────────

  const urlSlice = useBuilderStore((s) => s.urlSlice);
  const authSlice = useBuilderStore((s) => s.authSlice);
  const testSlice = useBuilderStore((s) => s.testSlice);
  const requestBodySlice = useBuilderStore((s) => s.requestBodySlice);

  const setTestResponse = useBuilderStore((s) => s.setTestResponse);
  const setTestStatusCode = useBuilderStore((s) => s.setTestStatusCode);
  const setTestOutcome = useBuilderStore((s) => s.setTestOutcome);
  const setIsUnverified = useBuilderStore((s) => s.setIsUnverified);
  const setSampleJson = useBuilderStore((s) => s.setSampleJson);
  const setStageValid = useBuilderStore((s) => s.setStageValid);

  // ── Local state ─────────────────────────────────────────────────────────────

  // Per-param value inputs for the test call (pre-filled from parsed URL).
  const [pathValues, setPathValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(urlSlice.pathParams.map((p) => [p.name, p.example])),
  );
  const [queryValues, setQueryValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(urlSlice.queryParams.map((p) => [p.key, p.rawValue])),
  );

  // Whether the sample JSON escape hatch is active.
  const [showSampleInput, setShowSampleInput] = useState(false);

  // ── Refs ────────────────────────────────────────────────────────────────────

  const abortRef = useRef<AbortController | null>(null);
  const timedOutRef = useRef(false);
  const userCancelledRef = useRef(false);
  /** Focusable heading above the Document Renderer — focused after success. */
  const rendererHeadingRef = useRef<HTMLHeadingElement>(null);

  // ── Auth ────────────────────────────────────────────────────────────────────

  const fetchAuth = useFetchWithAuth();

  // ── Mutation ────────────────────────────────────────────────────────────────

  const mutation = useMutation<TestCallResult, Error, TestCallPayload>({
    mutationFn: async (payload) => {
      const controller = new AbortController();
      abortRef.current = controller;
      timedOutRef.current = false;
      userCancelledRef.current = false;

      // 30-second client-side timeout (AC requirement).
      const timer = setTimeout(() => {
        timedOutRef.current = true;
        controller.abort();
      }, 30_000);

      try {
        const response = await fetchAuth('/api/v1/proxy/test', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
          signal: controller.signal,
        });

        clearTimeout(timer);

        if (response.status >= 200 && response.status < 300) {
          const data: unknown = await response.json().catch(() => null);
          return { outcome: 'success', statusCode: response.status, data };
        }

        if (response.status >= 400 && response.status < 500) {
          const data: unknown = await response.json().catch(() => null);
          return {
            outcome: '4xx',
            statusCode: response.status,
            statusText: response.statusText,
            data,
          };
        }

        return {
          outcome: '5xx',
          statusCode: response.status,
          statusText: response.statusText,
        };
      } catch {
        clearTimeout(timer);

        // Timeout takes priority: the timer fired and aborted the controller.
        if (timedOutRef.current) {
          return { outcome: 'timeout' };
        }

        // User-initiated cancel: treated as idle — the UI resets via mutation.reset().
        if (userCancelledRef.current) {
          return { outcome: 'cancelled' };
        }

        // Genuine network error (DNS failure, refused connection, etc.).
        try {
          const parsed = new URL(payload.url);
          return { outcome: 'network-error', host: parsed.hostname };
        } catch {
          return { outcome: 'network-error', host: payload.url };
        }
      }
    },
    retry: false,
  });

  // ── Side effects ────────────────────────────────────────────────────────────

  /** Sync mutation result to Zustand and handle side effects like focus. */
  useEffect(() => {
    const result = mutation.data;
    if (result === undefined) return;

    if (result.outcome === 'success') {
      setTestResponse(result.data);
      setTestStatusCode(result.statusCode);
      setTestOutcome('success');
      setIsUnverified(false);
      setStageValid('test', true);
      // Move focus into the renderer section per AC.
      setTimeout(() => {
        rendererHeadingRef.current?.focus();
      }, 0);
    } else if (result.outcome === 'timeout') {
      setTestOutcome('timeout');
      setStageValid('test', false);
    } else if (result.outcome === 'network-error') {
      setTestOutcome('network-error');
      setStageValid('test', false);
    } else if (result.outcome === '4xx' || result.outcome === '5xx') {
      setTestOutcome('error');
      setStageValid('test', false);
    }
    // 'cancelled' intentionally not synced — mutation.reset() handles UI state.
  }, [
    mutation.data,
    setTestResponse,
    setTestStatusCode,
    setTestOutcome,
    setIsUnverified,
    setStageValid,
  ]);

  // ── Handlers ────────────────────────────────────────────────────────────────

  const handleTest = useCallback(() => {
    const auth: TestCallPayload['auth'] = { type: authSlice.type };

    if (authSlice.type === 'bearer') {
      auth.bearerToken = authSlice.bearerToken;
    } else if (authSlice.type === 'api-key') {
      auth.apiKeyName = authSlice.apiKeyName;
      auth.apiKeyValue = authSlice.apiKeyValue;
      auth.apiKeyPlacement = authSlice.apiKeyPlacement;
    } else if (authSlice.type === 'basic') {
      auth.basicUsername = authSlice.basicUsername;
      auth.basicPassword = authSlice.basicPassword;
    }

    const payload: TestCallPayload = {
      url: urlSlice.url,
      method: urlSlice.method,
      pathParams: pathValues,
      queryParams: queryValues,
      auth,
    };

    if (requestBodySlice.requestBody !== null) {
      payload.body = requestBodySlice.requestBody;
    }

    mutation.mutate(payload);
  }, [urlSlice, authSlice, pathValues, queryValues, requestBodySlice, mutation]);

  const handleCancel = () => {
    userCancelledRef.current = true;
    abortRef.current?.abort();
    mutation.reset();
  };

  const handleActivateSampleMode = () => {
    setShowSampleInput(true);
  };

  const handleDeactivateSampleMode = () => {
    setShowSampleInput(false);
    setSampleJson(null);
    setIsUnverified(false);
    setStageValid('test', mutation.data?.outcome === 'success');
  };

  /** Called by SampleInputArea when the user confirms a raw JSON string. */
  const handleUseSample = (rawJson: string) => {
    if (rawJson.trim().length === 0) return;
    setSampleJson(rawJson);
    setIsUnverified(true);
    setStageValid('test', true);
  };

  // ── Derived state ───────────────────────────────────────────────────────────

  const isRunning = mutation.isPending;
  const result = mutation.data;
  const hasVisibleResult =
    result !== undefined && result.outcome !== 'cancelled';
  // hasVisibleResult already excludes 'cancelled', so only check for 'success'.
  const isFailure = hasVisibleResult && result.outcome !== 'success';
  const canProceed = testSlice.outcome === 'success' || testSlice.isUnverified;

  // ── Render ──────────────────────────────────────────────────────────────────

  return (
    <div className="space-y-24">
      {/* ── Param inputs ─────────────────────────────────────────────────── */}

      {urlSlice.pathParams.length > 0 && (
        <fieldset className="space-y-12">
          <legend className="text-sm font-medium text-text-primary">
            Path parameters
          </legend>
          <div className="space-y-8">
            {urlSlice.pathParams.map((param) => (
              <div key={param.name} className="flex items-center gap-12">
                <code className="w-1/3 truncate rounded bg-surface-container px-8 py-4 text-xs text-text-secondary">
                  {`{${param.name}}`}
                </code>
                <input
                  type="text"
                  value={pathValues[param.name] ?? ''}
                  onChange={(e) =>
                    setPathValues((prev) => ({
                      ...prev,
                      [param.name]: e.target.value,
                    }))
                  }
                  placeholder={`Value for ${param.name}`}
                  className="flex-1 rounded-md border border-border-default bg-surface-container-lowest px-12 py-8 text-sm text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-2 focus:ring-brand-500"
                />
              </div>
            ))}
          </div>
        </fieldset>
      )}

      {urlSlice.queryParams.length > 0 && (
        <fieldset className="space-y-12">
          <legend className="text-sm font-medium text-text-primary">
            Query parameters
          </legend>
          <div className="space-y-8">
            {urlSlice.queryParams.map((param) => (
              <div key={param.key} className="flex items-center gap-12">
                <code className="w-1/3 truncate rounded bg-surface-container px-8 py-4 text-xs text-text-secondary">
                  {param.key}
                </code>
                <input
                  type="text"
                  value={queryValues[param.key] ?? ''}
                  onChange={(e) =>
                    setQueryValues((prev) => ({
                      ...prev,
                      [param.key]: e.target.value,
                    }))
                  }
                  placeholder={`Value for ${param.key}`}
                  className="flex-1 rounded-md border border-border-default bg-surface-container-lowest px-12 py-8 text-sm text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-2 focus:ring-brand-500"
                />
              </div>
            ))}
          </div>
        </fieldset>
      )}

      {/* ── Test button row ───────────────────────────────────────────────── */}

      <div className="flex items-center gap-16">
        <button
          type="button"
          data-testid="test-call-btn"
          disabled={isRunning}
          onClick={handleTest}
          className="flex items-center gap-8 rounded-md bg-brand-500 px-20 py-10 text-sm font-medium text-primary-on hover:bg-brand-600 disabled:cursor-not-allowed disabled:opacity-60"
        >
          {isRunning ? (
            <>
              <Spinner />
              Testing…
            </>
          ) : (
            'Test'
          )}
        </button>

        {isRunning && (
          <button
            type="button"
            data-testid="test-call-cancel"
            onClick={handleCancel}
            className="text-sm text-text-secondary hover:text-text-primary hover:underline"
          >
            Cancel
          </button>
        )}
      </div>

      {/* ── Results area ─────────────────────────────────────────────────── */}

      {showSampleInput ? (
        <SampleInputArea
          onUse={handleUseSample}
          onBack={handleDeactivateSampleMode}
          isUnverified={testSlice.isUnverified}
        />
      ) : (
        hasVisibleResult && (
          <div className="space-y-16 rounded-lg border border-border-default p-16">
            {result.outcome === 'success' && (
              <>
                <div className="flex items-center gap-8">
                  <StatusIcon ok={true} />
                  <span className="text-sm font-medium text-emerald-700 dark:text-emerald-400">
                    Status {result.statusCode}
                  </span>
                </div>
                <h3
                  ref={rendererHeadingRef}
                  tabIndex={-1}
                  className="text-sm font-medium text-text-primary focus:outline-none"
                >
                  Response
                </h3>
                <DocumentRenderer
                  data={result.data}
                  selectedPaths={new Set<string>()}
                  onFieldSelect={() => {
                    /* field selection handled in mapping step */
                  }}
                />
              </>
            )}

            {result.outcome === '4xx' && (
              <>
                <div className="flex items-center gap-8 rounded-md bg-red-50 px-12 py-10 dark:bg-red-950">
                  <StatusIcon ok={false} />
                  <p className="text-sm font-medium text-red-700 dark:text-red-400">
                    API returned {result.statusCode}: {result.statusText}
                  </p>
                </div>
                {result.data !== null && (
                  <>
                    <p className="text-xs text-text-secondary">Response body:</p>
                    <DocumentRenderer
                      data={result.data}
                      selectedPaths={new Set<string>()}
                      onFieldSelect={() => {}}
                    />
                  </>
                )}
                <p className="text-sm text-text-secondary">
                  Try editing the parameter values above and running the test again.
                </p>
              </>
            )}

            {result.outcome === '5xx' && (
              <div className="flex items-center gap-12 rounded-md bg-red-50 px-12 py-10 dark:bg-red-950">
                <StatusIcon ok={false} />
                <div className="flex-1">
                  <p className="text-sm font-medium text-red-700 dark:text-red-400">
                    The upstream API returned a server error ({result.statusCode})
                  </p>
                </div>
                <button
                  type="button"
                  onClick={handleTest}
                  className="shrink-0 rounded-md bg-red-100 px-12 py-6 text-xs font-medium text-red-700 hover:bg-red-200 dark:bg-red-900 dark:text-red-300"
                >
                  Retry
                </button>
              </div>
            )}

            {result.outcome === 'timeout' && (
              <div className="rounded-md bg-amber-50 px-12 py-10 dark:bg-amber-950">
                <p className="text-sm font-medium text-amber-800 dark:text-amber-200">
                  The request timed out after 30 seconds
                </p>
                <div className="mt-8 flex gap-12 text-sm">
                  <button
                    type="button"
                    onClick={handleTest}
                    className="text-brand-500 hover:underline"
                  >
                    Try again
                  </button>
                  <button
                    type="button"
                    onClick={handleActivateSampleMode}
                    className="text-text-secondary hover:text-text-primary hover:underline"
                  >
                    Use sample response instead
                  </button>
                </div>
              </div>
            )}

            {result.outcome === 'network-error' && (
              <div className="rounded-md bg-red-50 px-12 py-10 dark:bg-red-950">
                <p className="text-sm font-medium text-red-700 dark:text-red-400">
                  Could not connect to{' '}
                  <code className="font-mono">{result.host}</code>
                </p>
                <div className="mt-8 flex gap-12 text-sm">
                  <button
                    type="button"
                    onClick={handleTest}
                    className="text-brand-500 hover:underline"
                  >
                    Try again
                  </button>
                  <button
                    type="button"
                    data-testid="use-sample-response-link"
                    onClick={handleActivateSampleMode}
                    className="text-text-secondary hover:text-text-primary hover:underline"
                  >
                    Use sample response instead
                  </button>
                </div>
              </div>
            )}
          </div>
        )
      )}

      {/* ── Navigation row ────────────────────────────────────────────────── */}

      <div className="flex items-center justify-between border-t border-border-default pt-16">
        <button
          type="button"
          onClick={onBack}
          className="text-sm text-brand-500 hover:underline"
        >
          ← Back
        </button>

        <div className="flex items-center gap-12">
          {isFailure && !showSampleInput && (
            <>
              <button
                type="button"
                onClick={handleActivateSampleMode}
                className="text-sm text-text-secondary hover:text-text-primary hover:underline"
              >
                Skip test, use sample response
              </button>
              <button
                type="button"
                onClick={handleTest}
                className="rounded-md border border-border-default bg-surface-container px-16 py-8 text-sm font-medium text-text-primary hover:bg-surface-container-high"
              >
                Fix and retry
              </button>
            </>
          )}

          {canProceed && (
            <button
              type="button"
              data-testid="proceed-to-mapping-btn"
              onClick={onContinue}
              className="rounded-md bg-brand-500 px-20 py-10 text-sm font-medium text-primary-on hover:bg-brand-600"
            >
              Continue to field mapping →
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

// ── SampleInputArea ────────────────────────────────────────────────────────────

/**
 * Minimal sample JSON escape hatch. TASK-008 adds JSON validation,
 * line number display, debounced validation, and error reporting.
 *
 * Owns its own uncontrolled textarea ref to avoid React 19 ref typing
 * complications when passing refs through props.
 */
interface SampleInputAreaProps {
  /** Called with the raw textarea content when the user clicks "Use this response". */
  onUse: (rawJson: string) => void;
  onBack: () => void;
  isUnverified: boolean;
}

function SampleInputArea({ onUse, onBack, isUnverified }: SampleInputAreaProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [hasContent, setHasContent] = useState(false);

  const handleUse = () => {
    const raw = textareaRef.current?.value ?? '';
    onUse(raw);
  };

  return (
    <div className="space-y-12 rounded-lg border border-border-default p-16">
      <div className="flex items-center justify-between">
        <p className="text-sm font-medium text-text-primary">
          Paste a sample API response
        </p>
        <button
          type="button"
          data-testid="back-to-live-test"
          onClick={onBack}
          className="text-xs text-text-secondary hover:text-text-primary hover:underline"
        >
          Clear and switch back to live test
        </button>
      </div>

      <textarea
        ref={textareaRef}
        data-testid="sample-json-input"
        rows={12}
        spellCheck={false}
        placeholder='{"key": "value"}'
        onChange={(e) => setHasContent(e.target.value.trim().length > 0)}
        className="w-full rounded-md border border-border-default bg-surface-container-lowest px-12 py-8 font-mono text-sm text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-2 focus:ring-brand-500"
      />

      {isUnverified && (
        <p
          role="status"
          className="text-xs font-medium text-amber-700 dark:text-amber-400"
        >
          Sample response — not live-tested
        </p>
      )}

      <button
        type="button"
        disabled={!hasContent}
        onClick={handleUse}
        className="rounded-md bg-brand-500 px-16 py-8 text-sm font-medium text-primary-on hover:bg-brand-600 disabled:cursor-not-allowed disabled:opacity-50"
      >
        Use this response
      </button>
    </div>
  );
}
