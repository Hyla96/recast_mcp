/**
 * UrlStep — step 1 of the builder flow.
 *
 * Renders the HTTP method selector + URL input, runs URL parsing (debounced
 * 150 ms), shows detected path/query parameters, and stores all state in
 * the Zustand builderStore.urlSlice.
 */

import { useEffect, useCallback, useRef } from 'react';
import type { ChangeEvent } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import type { HttpMethod, ParamType } from '@stores/builderStore';
import { useDebounce } from '@hooks/useDebounce';
import { parseRestUrl } from '@/lib/urlParser';
import type { ParsedUrl, UrlErrorCode } from '@/lib/urlParser';
import { RequestBodyBuilder } from '@components/builder/RequestBodyBuilder';

// ── Constants ────────────────────────────────────────────────────────────────

const HTTP_METHODS: HttpMethod[] = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH'];

const PARAM_TYPES: ParamType[] = ['string', 'number', 'boolean'];

const ERROR_MESSAGES: Record<UrlErrorCode, string> = {
  EMPTY: '',
  INVALID_URL: 'This URL is not valid. Check for typos.',
  NO_PROTOCOL: 'Add a protocol — for example: https://api.example.com/…',
  RELATIVE_URL: 'Enter a full URL starting with https:// or http://',
  UNSUPPORTED_PROTOCOL: 'Only http:// and https:// URLs are supported.',
};

// ── Component ─────────────────────────────────────────────────────────────────

// HTTP methods that send a request body.
const BODY_METHODS = new Set<HttpMethod>(['POST', 'PUT', 'PATCH']);

export function UrlStep({ onContinue }: { onContinue: () => void }) {
  const url = useBuilderStore((s) => s.urlSlice.url);
  const method = useBuilderStore((s) => s.urlSlice.method);
  const pathParams = useBuilderStore((s) => s.urlSlice.pathParams);
  const queryParams = useBuilderStore((s) => s.urlSlice.queryParams);
  const isValid = useBuilderStore((s) => s.urlSlice.isValid);

  const setUrl = useBuilderStore((s) => s.setUrl);
  const setMethod = useBuilderStore((s) => s.setMethod);
  const setPathParams = useBuilderStore((s) => s.setPathParams);
  const setQueryParams = useBuilderStore((s) => s.setQueryParams);
  const setUrlValid = useBuilderStore((s) => s.setUrlValid);

  // Persists the last body content across method switches so that switching
  // from POST → GET → POST restores the previously typed body.
  const savedBodyRef = useRef<string>('');

  // Stable callback for RequestBodyBuilder to report content changes.
  const handleBodyContentChange = useCallback((content: string) => {
    savedBodyRef.current = content;
  }, []);

  // Debounce the raw URL input before running the (synchronous but reactive)
  // parser.  150 ms matches the acceptance criteria.
  const debouncedUrl = useDebounce(url, 150);

  // Run the parser whenever the debounced URL changes and sync results into
  // the store, preserving any user edits to type/example on existing params.
  useEffect(() => {
    const parsed: ParsedUrl = parseRestUrl(debouncedUrl);

    if (parsed.isValid) {
      // Merge parser output with existing store params so user-edited type /
      // example values survive URL edits that don't rename the param.
      const mergedPath = parsed.pathParams.map((pp) => {
        const existing = pathParams.find((ep) => ep.name === pp.name);
        return existing !== undefined
          ? { ...pp, type: existing.type, example: existing.example }
          : pp;
      });

      const mergedQuery = parsed.queryParams.map((qp) => {
        const existing = queryParams.find((eq) => eq.key === qp.key);
        return existing !== undefined
          ? { ...qp, type: existing.type, rawValue: existing.rawValue }
          : qp;
      });

      setPathParams(mergedPath);
      setQueryParams(mergedQuery);
    } else {
      // Clear params on invalid URL so the detected-params section hides.
      setPathParams([]);
      setQueryParams([]);
    }

    setUrlValid(parsed.isValid);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debouncedUrl]);
  // Intentionally omitting pathParams/queryParams from deps — we read them
  // as a snapshot for the merge; including them would cause infinite loops.

  // Determine the error message to show.
  const parsed = parseRestUrl(debouncedUrl);
  const showError = !parsed.isValid && parsed.error !== 'EMPTY' && debouncedUrl.trim() !== '';
  const errorMessage = parsed.error !== undefined ? ERROR_MESSAGES[parsed.error] : '';

  const hasParams = pathParams.length > 0 || queryParams.length > 0;
  const showParamsSection = debouncedUrl.trim() !== '' && parsed.isValid;

  // ── Handlers ──────────────────────────────────────────────────────────────

  const handleUrlChange = useCallback(
    (e: ChangeEvent<HTMLInputElement>) => {
      setUrl(e.target.value);
    },
    [setUrl]
  );

  const handleMethodChange = useCallback(
    (e: ChangeEvent<HTMLSelectElement>) => {
      setMethod(e.target.value as HttpMethod);
    },
    [setMethod]
  );

  const handlePathParamTypeChange = useCallback(
    (name: string, type: ParamType) => {
      const updated = pathParams.map((p) => (p.name === name ? { ...p, type } : p));
      setPathParams(updated);
    },
    [pathParams, setPathParams]
  );

  const handlePathParamExampleChange = useCallback(
    (name: string, example: string) => {
      const updated = pathParams.map((p) => (p.name === name ? { ...p, example } : p));
      setPathParams(updated);
    },
    [pathParams, setPathParams]
  );

  const handleQueryParamKeyChange = useCallback(
    (index: number, key: string) => {
      const updated = queryParams.map((q, i) => (i === index ? { ...q, key } : q));
      setQueryParams(updated);
    },
    [queryParams, setQueryParams]
  );

  const handleQueryParamTypeChange = useCallback(
    (index: number, type: ParamType) => {
      const updated = queryParams.map((q, i) => (i === index ? { ...q, type } : q));
      setQueryParams(updated);
    },
    [queryParams, setQueryParams]
  );

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <div className="space-y-32">
      {/* URL + method row */}
      <div className="space-y-8">
        <label className="block text-sm font-medium text-text-primary" htmlFor="url-input">
          API endpoint URL
        </label>
        <div className="flex gap-8">
          {/* Method selector */}
          <select
            data-testid="method-select"
            value={method}
            onChange={handleMethodChange}
            className="rounded-md border border-border-default bg-surface-container-lowest px-12 py-10 text-sm font-medium text-text-primary focus:outline-none focus:ring-2 focus:ring-brand-500 focus:border-brand-500"
            aria-label="HTTP method"
          >
            {HTTP_METHODS.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>

          {/* URL input */}
          <input
            id="url-input"
            data-testid="url-input"
            type="url"
            value={url}
            onChange={handleUrlChange}
            placeholder="https://api.example.com/v1/users/{userId}"
            autoComplete="off"
            spellCheck={false}
            className={`flex-1 rounded-md border bg-surface-container-lowest px-12 py-10 text-sm text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-2 focus:ring-brand-500 transition-colors ${
              showError
                ? 'border-error-DEFAULT ring-1 ring-error-DEFAULT'
                : 'border-border-default focus:border-brand-500'
            }`}
          />
        </div>

        {/* Inline error */}
        {showError && (
          <p className="text-sm text-error-DEFAULT" role="alert">
            {errorMessage}
          </p>
        )}
      </div>

      {/* Detected parameters section */}
      {showParamsSection && (
        <div className="space-y-16">
          <h3 className="text-sm font-medium text-text-primary">Detected parameters</h3>

          {!hasParams && (
            <p className="text-sm text-text-secondary">No parameters detected</p>
          )}

          {/* Path params */}
          {pathParams.length > 0 && (
            <div className="space-y-8">
              <p className="text-xs font-medium text-text-secondary uppercase tracking-wide">
                Path parameters
              </p>
              {pathParams.map((param) => (
                <div
                  key={param.name}
                  className="flex items-center gap-12 rounded-md bg-surface-container p-12"
                >
                  {/* Tag */}
                  <span className="shrink-0 rounded-sm bg-brand-100 px-8 py-2 text-xs font-medium text-brand-600">
                    Path param
                  </span>

                  {/* Name */}
                  <code className="shrink-0 text-sm font-mono text-text-primary">
                    {param.name}
                  </code>

                  {/* Type selector */}
                  <select
                    value={param.type}
                    onChange={(e) =>
                      handlePathParamTypeChange(param.name, e.target.value as ParamType)
                    }
                    className="rounded border border-border-default bg-surface-container-lowest px-8 py-4 text-xs text-text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
                    aria-label={`Type for path param ${param.name}`}
                  >
                    {PARAM_TYPES.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </select>

                  {/* Example value */}
                  <input
                    type="text"
                    value={param.example}
                    onChange={(e) =>
                      handlePathParamExampleChange(param.name, e.target.value)
                    }
                    placeholder="example value"
                    className="flex-1 rounded border border-border-default bg-surface-container-lowest px-8 py-4 text-xs text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-1 focus:ring-brand-500"
                    aria-label={`Example value for path param ${param.name}`}
                  />
                </div>
              ))}
            </div>
          )}

          {/* Query params */}
          {queryParams.length > 0 && (
            <div className="space-y-8">
              <p className="text-xs font-medium text-text-secondary uppercase tracking-wide">
                Query parameters
              </p>
              {queryParams.map((param, index) => (
                <div
                  key={index}
                  className="flex items-center gap-12 rounded-md bg-surface-container p-12"
                >
                  {/* Tag */}
                  <span className="shrink-0 rounded-sm bg-surface-variant px-8 py-2 text-xs font-medium text-text-secondary">
                    Query param
                  </span>

                  {/* Editable key name */}
                  <input
                    type="text"
                    value={param.key}
                    onChange={(e) => handleQueryParamKeyChange(index, e.target.value)}
                    className="w-32 rounded border border-border-default bg-surface-container-lowest px-8 py-4 text-xs font-mono text-text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
                    aria-label={`Key name for query param ${index + 1}`}
                  />

                  {/* Type selector */}
                  <select
                    value={param.type}
                    onChange={(e) =>
                      handleQueryParamTypeChange(index, e.target.value as ParamType)
                    }
                    className="rounded border border-border-default bg-surface-container-lowest px-8 py-4 text-xs text-text-primary focus:outline-none focus:ring-1 focus:ring-brand-500"
                    aria-label={`Type for query param ${param.key}`}
                  >
                    {PARAM_TYPES.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </select>

                  {/* Raw value (read-only display, pre-filled from URL) */}
                  <span className="flex-1 truncate text-xs text-text-secondary font-mono">
                    {param.rawValue !== '' ? param.rawValue : <em>empty</em>}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Request body builder — visible only for body-carrying methods */}
      {BODY_METHODS.has(method) && (
        <RequestBodyBuilder
          initialContent={savedBodyRef.current}
          onContentChange={handleBodyContentChange}
        />
      )}

      {/* Continue button */}
      <div className="flex justify-end">
        <button
          data-testid="url-step-continue"
          type="button"
          disabled={!isValid}
          onClick={onContinue}
          className="rounded-md bg-brand-500 px-24 py-10 text-sm font-medium text-primary-on transition-colors hover:bg-brand-600 disabled:cursor-not-allowed disabled:opacity-40 focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-2"
        >
          Continue
        </button>
      </div>
    </div>
  );
}
