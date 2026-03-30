/**
 * AuthStep — step 2 of the builder flow.
 *
 * Lets the user choose an auth type (None / Bearer Token / API Key /
 * Basic Auth) and configure credentials.  All credential values are stored
 * in Zustand memory only — never persisted to localStorage or sessionStorage
 * (the builderStore auth slice is excluded from the persist middleware).
 */

import { useState, useEffect } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import type { AuthType, ApiKeyPlacement } from '@stores/builderStore';
import { SegmentedControl } from '@components/builder/SegmentedControl';
import { PasswordInput } from '@components/builder/PasswordInput';
import { EncryptedFieldBadge } from '@components/builder/EncryptedFieldBadge';

// ── Constants ─────────────────────────────────────────────────────────────────

const AUTH_OPTIONS: Array<{ value: AuthType; label: string }> = [
  { value: 'none', label: 'None' },
  { value: 'bearer', label: 'Bearer Token' },
  { value: 'api-key', label: 'API Key' },
  { value: 'basic', label: 'Basic Auth' },
];

const PLACEMENT_OPTIONS: Array<{ value: ApiKeyPlacement; label: string }> = [
  { value: 'header', label: 'Header' },
  { value: 'query', label: 'Query' },
];

/** Common API key header names offered as datalist suggestions. */
const COMMON_HEADER_NAMES = [
  'Authorization',
  'X-API-Key',
  'X-Auth-Token',
  'X-Access-Token',
  'Api-Key',
  'Token',
];

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Compute whether the Continue button should be enabled for a given auth config. */
function computeCanContinue(
  authType: AuthType,
  bearerToken: string,
  apiKeyName: string,
  apiKeyValue: string,
  basicUsername: string,
  basicPassword: string
): boolean {
  switch (authType) {
    case 'none':
      return true;
    case 'bearer':
      return bearerToken.trim().length >= 10;
    case 'api-key':
      return apiKeyName.trim().length > 0 && apiKeyValue.trim().length > 0;
    case 'basic':
      return basicUsername.trim().length > 0 && basicPassword.trim().length > 0;
  }
}

// ── Component ─────────────────────────────────────────────────────────────────

export function AuthStep({
  onContinue,
  onBack,
}: {
  onContinue: () => void;
  onBack: () => void;
}) {
  // ── Store reads ───────────────────────────────────────────────────────────

  const authType = useBuilderStore((s) => s.authSlice.type);
  const bearerToken = useBuilderStore((s) => s.authSlice.bearerToken);
  const apiKeyName = useBuilderStore((s) => s.authSlice.apiKeyName);
  const apiKeyValue = useBuilderStore((s) => s.authSlice.apiKeyValue);
  const apiKeyPlacement = useBuilderStore((s) => s.authSlice.apiKeyPlacement);
  const basicUsername = useBuilderStore((s) => s.authSlice.basicUsername);
  const basicPassword = useBuilderStore((s) => s.authSlice.basicPassword);

  const setAuthType = useBuilderStore((s) => s.setAuthType);
  const setBearerToken = useBuilderStore((s) => s.setBearerToken);
  const setApiKeyName = useBuilderStore((s) => s.setApiKeyName);
  const setApiKeyValue = useBuilderStore((s) => s.setApiKeyValue);
  const setApiKeyPlacement = useBuilderStore((s) => s.setApiKeyPlacement);
  const setBasicUsername = useBuilderStore((s) => s.setBasicUsername);
  const setBasicPassword = useBuilderStore((s) => s.setBasicPassword);
  const setStageValid = useBuilderStore((s) => s.setStageValid);

  // ── Blur-triggered validation state ─────────────────────────────────────
  // Only show errors after the user has touched a field (blur).

  const [bearerBlurred, setBearerBlurred] = useState(false);
  const [apiKeyNameBlurred, setApiKeyNameBlurred] = useState(false);
  const [usernameBlurred, setUsernameBlurred] = useState(false);
  const [passwordBlurred, setPasswordBlurred] = useState(false);

  // ── Auth type change ──────────────────────────────────────────────────────
  // Reset all blur state when the user switches auth type so previously
  // touched fields from another type don't show stale errors.

  const handleAuthTypeChange = (type: AuthType) => {
    setAuthType(type);
    setBearerBlurred(false);
    setApiKeyNameBlurred(false);
    setUsernameBlurred(false);
    setPasswordBlurred(false);
  };

  // ── Derived validation ────────────────────────────────────────────────────

  const bearerTooShort = bearerBlurred && bearerToken.trim().length < 10;
  const apiKeyNameMissing = apiKeyNameBlurred && apiKeyName.trim().length === 0;
  const usernameMissing = usernameBlurred && basicUsername.trim().length === 0;
  const passwordMissing = passwordBlurred && basicPassword.trim().length === 0;

  const canContinue = computeCanContinue(
    authType,
    bearerToken,
    apiKeyName,
    apiKeyValue,
    basicUsername,
    basicPassword
  );

  // Keep the store's stageValidation in sync so parent and future steps
  // can accurately reflect whether the auth stage is complete.
  useEffect(() => {
    setStageValid('auth', canContinue);
  }, [canContinue, setStageValid]);

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <div data-testid="auth-panel" className="space-y-32">
      {/* Auth type selector */}
      <div className="space-y-8">
        <p className="text-sm font-medium text-text-primary">Authentication type</p>
        <div data-testid="auth-type-selector">
          <SegmentedControl
            options={AUTH_OPTIONS}
            value={authType}
            onChange={handleAuthTypeChange}
            name="auth-type"
            aria-label="Authentication type"
          />
        </div>
      </div>

      {/* ── None: amber advisory ──────────────────────────────────────── */}
      {authType === 'none' && (
        <div
          role="status"
          aria-live="polite"
          className="flex items-start gap-10 rounded-md border border-amber-200 bg-amber-50 px-16 py-12 text-sm text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200"
        >
          <WarningIcon className="mt-1 h-16 w-16 shrink-0" aria-hidden="true" />
          <span>
            No authentication configured. Requests will be sent without credentials. Only
            use this for public APIs.
          </span>
        </div>
      )}

      {/* ── Bearer Token ─────────────────────────────────────────────── */}
      {authType === 'bearer' && (
        <div className="space-y-8">
          <div className="flex items-center justify-between">
            <label htmlFor="bearer-token" className="text-sm font-medium text-text-primary">
              Bearer token
            </label>
            <EncryptedFieldBadge />
          </div>
          <PasswordInput
            id="bearer-token"
            value={bearerToken}
            onChange={setBearerToken}
            onBlur={() => setBearerBlurred(true)}
            placeholder="Enter your bearer token"
            autoComplete="new-password"
            aria-describedby={bearerTooShort ? 'bearer-token-error' : undefined}
          />
          {bearerTooShort && (
            <p id="bearer-token-error" className="text-sm text-error-DEFAULT" role="alert">
              Token must be at least 10 characters.
            </p>
          )}
        </div>
      )}

      {/* ── API Key ──────────────────────────────────────────────────── */}
      {authType === 'api-key' && (
        <div className="space-y-24">
          {/* Placement toggle */}
          <div className="space-y-8">
            <p className="text-sm font-medium text-text-primary">Send as</p>
            <div data-testid="apikey-placement-toggle">
              <SegmentedControl
                options={PLACEMENT_OPTIONS}
                value={apiKeyPlacement}
                onChange={setApiKeyPlacement}
                name="apikey-placement"
                aria-label="API key placement"
              />
            </div>
          </div>

          {/* Key name */}
          <div className="space-y-8">
            <label htmlFor="apikey-name" className="block text-sm font-medium text-text-primary">
              {apiKeyPlacement === 'header' ? 'Header name' : 'Parameter name'}
            </label>
            <input
              id="apikey-name"
              type="text"
              value={apiKeyName}
              onChange={(e) => setApiKeyName(e.target.value)}
              onBlur={() => setApiKeyNameBlurred(true)}
              list="apikey-name-suggestions"
              autoComplete="off"
              placeholder={apiKeyPlacement === 'header' ? 'X-API-Key' : 'api_key'}
              aria-describedby={apiKeyNameMissing ? 'apikey-name-error' : undefined}
              className={`w-full rounded-md border bg-surface-container-lowest px-12 py-8 text-sm text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-2 focus:ring-brand-500 transition-colors ${
                apiKeyNameMissing
                  ? 'border-error-DEFAULT ring-1 ring-error-DEFAULT'
                  : 'border-border-default focus:border-brand-500'
              }`}
            />
            <datalist id="apikey-name-suggestions">
              {COMMON_HEADER_NAMES.map((name) => (
                <option key={name} value={name} />
              ))}
            </datalist>
            {apiKeyNameMissing && (
              <p id="apikey-name-error" className="text-sm text-error-DEFAULT" role="alert">
                Key name is required.
              </p>
            )}
          </div>

          {/* Key value */}
          <div className="space-y-8">
            <div className="flex items-center justify-between">
              <label htmlFor="apikey-value" className="text-sm font-medium text-text-primary">
                Key value
              </label>
              <EncryptedFieldBadge />
            </div>
            <PasswordInput
              id="apikey-value"
              value={apiKeyValue}
              onChange={setApiKeyValue}
              placeholder="Enter your API key"
              autoComplete="new-password"
            />
          </div>
        </div>
      )}

      {/* ── Basic Auth ───────────────────────────────────────────────── */}
      {authType === 'basic' && (
        <div className="space-y-24">
          {/* Username */}
          <div className="space-y-8">
            <label
              htmlFor="basic-username"
              className="block text-sm font-medium text-text-primary"
            >
              Username
            </label>
            <input
              id="basic-username"
              type="text"
              value={basicUsername}
              onChange={(e) => setBasicUsername(e.target.value)}
              onBlur={() => setUsernameBlurred(true)}
              autoComplete="off"
              placeholder="username"
              aria-describedby={usernameMissing ? 'basic-username-error' : undefined}
              className={`w-full rounded-md border bg-surface-container-lowest px-12 py-8 text-sm text-text-primary placeholder:text-text-secondary focus:outline-none focus:ring-2 focus:ring-brand-500 transition-colors ${
                usernameMissing
                  ? 'border-error-DEFAULT ring-1 ring-error-DEFAULT'
                  : 'border-border-default focus:border-brand-500'
              }`}
            />
            {usernameMissing && (
              <p id="basic-username-error" className="text-sm text-error-DEFAULT" role="alert">
                Username is required.
              </p>
            )}
          </div>

          {/* Password */}
          <div className="space-y-8">
            <div className="flex items-center justify-between">
              <label
                htmlFor="basic-password"
                className="text-sm font-medium text-text-primary"
              >
                Password
              </label>
              <EncryptedFieldBadge />
            </div>
            <PasswordInput
              id="basic-password"
              value={basicPassword}
              onChange={setBasicPassword}
              onBlur={() => setPasswordBlurred(true)}
              placeholder="password"
              autoComplete="new-password"
              aria-describedby={passwordMissing ? 'basic-password-error' : undefined}
            />
            {passwordMissing && (
              <p id="basic-password-error" className="text-sm text-error-DEFAULT" role="alert">
                Password is required.
              </p>
            )}
          </div>
        </div>
      )}

      {/* Navigation */}
      <div className="flex items-center justify-between pt-8 border-t border-border-subtle">
        <button
          type="button"
          onClick={onBack}
          className="text-sm text-brand-500 hover:underline focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
        >
          ← Back
        </button>
        <button
          type="button"
          disabled={!canContinue}
          onClick={onContinue}
          className="rounded-md bg-brand-500 px-24 py-10 text-sm font-medium text-primary-on transition-colors hover:bg-brand-600 disabled:cursor-not-allowed disabled:opacity-40 focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-2"
        >
          Continue
        </button>
      </div>
    </div>
  );
}

// ── Icon ──────────────────────────────────────────────────────────────────────

function WarningIcon({ className, 'aria-hidden': ariaHidden }: { className?: string; 'aria-hidden'?: boolean | 'true' | 'false' }) {
  return (
    <svg
      className={className}
      aria-hidden={ariaHidden}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
      <line x1="12" y1="9" x2="12" y2="13" />
      <line x1="12" y1="17" x2="12.01" y2="17" />
    </svg>
  );
}
