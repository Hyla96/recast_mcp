/**
 * NamingStep — step 5 of the builder flow.
 *
 * Lets the user assign a tool name (used by Claude to identify the tool) and
 * an optional description. Validates the name against naming rules in real
 * time and optionally warns when a tool with the same name already exists.
 */

import { useState, useEffect, useRef, useId } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useBuilderStore } from '@stores/builderStore';
import { useFetchWithAuth } from '@/lib/fetchWithAuth';
import { useDebounce } from '@hooks/useDebounce';
import { generateToolName, validateToolName, filterToolNameChars } from '@/lib/toolNameUtils';

// ─── Constants ────────────────────────────────────────────────────────────────

const MAX_DESC_CHARS = 500;
const DESC_ANNOUNCE_THRESHOLDS = [400, 450, 475, 500];

// ─── Helpers ──────────────────────────────────────────────────────────────────

/** Returns the highest threshold that has been reached (or null). */
function currentThreshold(len: number): number | null {
  let result: number | null = null;
  for (const t of DESC_ANNOUNCE_THRESHOLDS) {
    if (len >= t) result = t;
  }
  return result;
}

// ─── Uniqueness check types ───────────────────────────────────────────────────

interface ServersCheckResponse {
  data: unknown[];
}

// ─── Component ────────────────────────────────────────────────────────────────

interface NamingStepProps {
  onContinue: () => void;
  onBack: () => void;
}

export function NamingStep({ onContinue, onBack }: NamingStepProps) {
  const url = useBuilderStore((s) => s.urlSlice.url);
  const toolName = useBuilderStore((s) => s.namingSlice.toolName);
  const toolDescription = useBuilderStore((s) => s.namingSlice.toolDescription);
  const setToolName = useBuilderStore((s) => s.setToolName);
  const setToolDescription = useBuilderStore((s) => s.setToolDescription);
  const setStageValid = useBuilderStore((s) => s.setStageValid);

  // Validation state shown after first blur
  const [hasBlurred, setHasBlurred] = useState(false);
  const [nameError, setNameError] = useState<string | null>(null);

  // Description character counter + a11y announcement
  const [announcement, setAnnouncement] = useState('');
  const prevThresholdRef = useRef<number | null>(null);

  // Ids for aria relationships
  const nameInputId = useId();
  const nameErrorId = useId();
  const descInputId = useId();
  const descCountId = useId();
  const announcementId = useId();

  const fetchAuth = useFetchWithAuth();

  // ── Auto-suggest on mount ────────────────────────────────────────────────────

  const hasSuggested = useRef(false);
  useEffect(() => {
    if (!hasSuggested.current && toolName === '') {
      hasSuggested.current = true;
      setToolName(generateToolName(url));
    }
    // Only run once on mount; deps intentionally empty after flag guard.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Derived validation ────────────────────────────────────────────────────────

  const nameValidationError = validateToolName(toolName);
  const nameIsValid = nameValidationError === null;

  // Sync stage validity with naming validation
  useEffect(() => {
    setStageValid('naming', nameIsValid);
  }, [nameIsValid, setStageValid]);

  // ── Uniqueness check (debounced 500ms) ───────────────────────────────────────

  const debouncedName = useDebounce(toolName, 500);

  const { data: nameCheckData } = useQuery<ServersCheckResponse>({
    queryKey: ['toolNameCheck', debouncedName],
    queryFn: async () => {
      const res = await fetchAuth(
        `/api/v1/servers?toolName=${encodeURIComponent(debouncedName)}`
      );
      return res.json() as Promise<ServersCheckResponse>;
    },
    enabled: nameIsValid && debouncedName.length >= 3,
    staleTime: 30_000,
    retry: false,
  });

  const nameAlreadyExists =
    Array.isArray(nameCheckData?.data) && nameCheckData.data.length > 0;

  // ── Input handlers ────────────────────────────────────────────────────────────

  const handleNameChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const filtered = filterToolNameChars(e.target.value);
    setToolName(filtered);
    if (hasBlurred) {
      setNameError(validateToolName(filtered));
    }
  };

  const handleNameBlur = () => {
    setHasBlurred(true);
    setNameError(validateToolName(toolName));
  };

  const handleDescriptionChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const val = e.target.value.slice(0, MAX_DESC_CHARS);
    setToolDescription(val);

    const len = val.length;
    const threshold = currentThreshold(len);
    if (threshold !== null && threshold !== prevThresholdRef.current) {
      prevThresholdRef.current = threshold;
      setAnnouncement(`${len} of ${MAX_DESC_CHARS} characters used`);
    }
  };

  // ── Render ────────────────────────────────────────────────────────────────────

  const showNameError = hasBlurred && nameError !== null;
  const descLen = toolDescription.length;
  const descNearLimit = descLen >= 400;

  return (
    <form data-testid="tool-naming-form" onSubmit={(e) => e.preventDefault()}>
      {/* ── Tool name ─────────────────────────────────────────────────────── */}
      <div>
        <label
          htmlFor={nameInputId}
          className="block text-sm font-medium text-text-primary"
        >
          Tool name
        </label>
        <p className="mt-4 text-xs text-text-secondary">
          Lowercase letters, digits, and underscores only (e.g.{' '}
          <code className="font-mono">get_weather</code>). Used by Claude to
          identify this tool.
        </p>

        <input
          id={nameInputId}
          type="text"
          data-testid="tool-name-input"
          value={toolName}
          onChange={handleNameChange}
          onBlur={handleNameBlur}
          maxLength={50}
          aria-invalid={showNameError}
          aria-describedby={showNameError ? nameErrorId : undefined}
          className={[
            'mt-8 block w-full rounded-md border bg-surface-container px-12 py-8',
            'font-mono text-sm text-text-primary placeholder:text-text-secondary',
            'focus:outline-none focus:ring-2',
            showNameError
              ? 'border-error ring-error focus:ring-error'
              : 'border-border focus:ring-brand-500',
          ].join(' ')}
          placeholder="e.g. get_weather"
          spellCheck={false}
          autoComplete="off"
        />

        {/* Inline validation error */}
        {showNameError && (
          <p
            id={nameErrorId}
            role="alert"
            className="mt-6 text-xs text-error"
          >
            {nameError}
          </p>
        )}

        {/* Uniqueness warning (non-blocking) */}
        {nameAlreadyExists && !showNameError && (
          <p className="mt-6 text-xs text-amber-600 dark:text-amber-400">
            You already have a tool named{' '}
            <span className="font-mono">{toolName}</span>. You can still
            continue — the new server will be separate.
          </p>
        )}
      </div>

      {/* ── Description ───────────────────────────────────────────────────── */}
      <div className="mt-24">
        <label
          htmlFor={descInputId}
          className="block text-sm font-medium text-text-primary"
        >
          Description{' '}
          <span className="font-normal text-text-secondary">(optional)</span>
        </label>
        <p className="mt-4 text-xs text-text-secondary">
          Tells Claude when and how to use this tool. Max 500 characters.
        </p>

        <textarea
          id={descInputId}
          data-testid="tool-description-input"
          value={toolDescription}
          onChange={handleDescriptionChange}
          rows={4}
          maxLength={MAX_DESC_CHARS}
          aria-describedby={`${descCountId} ${announcementId}`}
          className={[
            'mt-8 block w-full rounded-md border border-border bg-surface-container',
            'px-12 py-8 text-sm text-text-primary placeholder:text-text-secondary',
            'focus:outline-none focus:ring-2 focus:ring-brand-500',
            'resize-y',
          ].join(' ')}
          placeholder="Describe what this tool does, e.g. 'Fetches current weather for a given city.'"
        />

        {/* Character counter */}
        <div className="mt-6 flex justify-end">
          <span
            id={descCountId}
            className={[
              'text-xs tabular-nums',
              descNearLimit ? 'text-amber-600 dark:text-amber-400' : 'text-text-secondary',
            ].join(' ')}
          >
            {descLen} / {MAX_DESC_CHARS}
          </span>
        </div>

        {/* Hidden aria-live region for threshold announcements */}
        <span
          id={announcementId}
          aria-live="polite"
          aria-atomic="true"
          className="sr-only"
        >
          {announcement}
        </span>
      </div>

      {/* ── Preview ───────────────────────────────────────────────────────── */}
      <div className="mt-24">
        <h3 className="text-sm font-medium text-text-primary">Preview</h3>
        <p className="mt-4 text-xs text-text-secondary">
          How this tool appears in Claude Desktop.
        </p>

        <ToolPreviewCard name={toolName} description={toolDescription} />
      </div>

      {/* ── Navigation ───────────────────────────────────────────────────── */}
      <div className="mt-32 flex items-center justify-between">
        <button
          type="button"
          onClick={onBack}
          className="text-sm text-brand-500 hover:underline focus:outline-none focus:underline"
        >
          ← Back
        </button>

        <button
          type="button"
          data-testid="tool-naming-continue"
          disabled={!nameIsValid}
          onClick={() => {
            if (nameIsValid) onContinue();
          }}
          title={!nameIsValid ? 'Fix the tool name to continue' : undefined}
          className="rounded-md bg-brand-500 px-24 py-10 text-sm font-medium text-primary-on hover:bg-brand-600 disabled:cursor-not-allowed disabled:opacity-40 focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-2"
        >
          Continue
        </button>
      </div>
    </form>
  );
}

// ─── Tool Preview Card ────────────────────────────────────────────────────────

interface ToolPreviewCardProps {
  name: string;
  description: string;
}

function ToolPreviewCard({ name, description }: ToolPreviewCardProps) {
  return (
    <div
      data-testid="tool-preview"
      className="mt-12 rounded-lg border border-border bg-surface-container-low p-16"
    >
      {/* Mock Claude Desktop tool card */}
      <div className="flex items-start gap-12">
        {/* Tool icon placeholder */}
        <div className="flex h-32 w-32 flex-shrink-0 items-center justify-center rounded-md bg-brand-100 dark:bg-brand-900">
          <svg
            className="h-18 w-18 text-brand-500"
            fill="none"
            viewBox="0 0 24 24"
            strokeWidth={1.5}
            stroke="currentColor"
            aria-hidden="true"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M11.42 15.17 17.25 21A2.652 2.652 0 0 0 21 17.25l-5.877-5.877M11.42 15.17l2.496-3.03c.317-.384.74-.626 1.208-.766M11.42 15.17l-4.655 5.653a2.548 2.548 0 1 1-3.586-3.586l6.837-5.63m5.108-.233c.55-.164 1.163-.188 1.743-.14a4.5 4.5 0 0 0 4.486-6.336l-3.276 3.277a3.004 3.004 0 0 1-2.25-2.25l3.276-3.276a4.5 4.5 0 0 0-6.336 4.486c.091 1.076-.071 2.264-.904 2.95l-.102.085m-1.745 1.437L5.909 7.5H4.5L2.25 3.75l1.5-1.5L7.5 4.5v1.409l4.26 4.26m-1.745 1.437 1.745-1.437m6.615 8.206L15.75 15.75M4.867 19.125h.008v.008h-.008v-.008Z"
            />
          </svg>
        </div>

        <div className="min-w-0 flex-1">
          <p className="font-mono text-sm font-medium text-text-primary">
            {name || <span className="italic text-text-secondary">tool_name</span>}
          </p>
          <p className="mt-4 text-xs text-text-secondary">
            {description.trim() !== '' ? (
              description
            ) : (
              <span className="italic">[No description provided]</span>
            )}
          </p>
        </div>
      </div>
    </div>
  );
}
