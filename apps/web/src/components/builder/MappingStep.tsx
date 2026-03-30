/**
 * MappingStep — step 4 of the builder flow.
 *
 * Renders the API response via DocumentRenderer on the left and the
 * SelectedFieldsPanel on the right (desktop: 60/40 split; tablet/mobile:
 * stacked). Clicking any value in the renderer toggles it in the selected
 * fields list.
 *
 * When the clicked value's JSONPath contains a numeric array index, the
 * ArrayNormalizationDialog is shown so the user can choose whether the
 * field should cover all array items (normalised path) or only the specific
 * clicked item (original path).
 */

import { useCallback, useEffect, useState } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import { DocumentRenderer } from '@components/builder/DocumentRenderer';
import { SelectedFieldsPanel } from '@components/builder/SelectedFieldsPanel';
import { ArrayNormalizationDialog } from '@components/builder/ArrayNormalizationDialog';
import { useSelectedPaths } from '@hooks/useSelectedPaths';
import {
  fieldNameFromJsonPath,
  typeFromValue,
  exampleFromValue,
} from '@/lib/fieldMappingUtils';
import { hasArrayIndex, normalizeArrayPath } from '@/lib/jsonPathUtils';

// ── Types ──────────────────────────────────────────────────────────────────────

interface MappingStepProps {
  onContinue: () => void;
  onBack: () => void;
}

/** State held while the normalisation dialog is visible. */
interface PendingField {
  jsonPath: string;
  value: unknown;
  /** The element that was focused when the click occurred; focus is returned here on dialog close. */
  triggerEl: HTMLElement | null;
}

// ── Helpers ────────────────────────────────────────────────────────────────────

function safeParseJson(raw: string | null): unknown {
  if (raw === null) return null;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return null;
  }
}

// ── Component ──────────────────────────────────────────────────────────────────

export function MappingStep({ onContinue, onBack }: MappingStepProps) {
  const testResponse = useBuilderStore((s) => s.testSlice.response);
  const sampleJson = useBuilderStore((s) => s.testSlice.sampleJson);
  const selectedFields = useBuilderStore((s) => s.mappingSlice.selectedFields);
  const addSelectedField = useBuilderStore((s) => s.addSelectedField);
  const removeSelectedField = useBuilderStore((s) => s.removeSelectedField);
  const setStageValid = useBuilderStore((s) => s.setStageValid);

  const selectedPaths = useSelectedPaths();

  // Pending field awaiting the normalisation dialog decision.
  const [pendingField, setPendingField] = useState<PendingField | null>(null);

  // Determine the response data to render.
  // sampleJson takes priority when set; otherwise fall back to the live test response.
  const responseData: unknown =
    sampleJson !== null ? safeParseJson(sampleJson) : testResponse;

  // Keep stageValidation.mapping in sync with field selection.
  useEffect(() => {
    setStageValid('mapping', selectedFields.length > 0);
  }, [selectedFields.length, setStageValid]);

  const canContinue = selectedFields.length > 0;

  // ── Field selection ─────────────────────────────────────────────────────

  const handleFieldSelect = useCallback(
    (jsonPath: string, value: unknown) => {
      const normalizedPath = normalizeArrayPath(jsonPath);

      // If this field is already selected (by original or normalised path), toggle it off.
      // removeSelectedField is a filter — calling it for a non-existent path is a no-op.
      if (selectedPaths.has(jsonPath) || selectedPaths.has(normalizedPath)) {
        removeSelectedField(jsonPath);
        removeSelectedField(normalizedPath);
        return;
      }

      // If the path contains a numeric array index, show the normalisation dialog
      // so the user can decide whether to cover all items or just this one.
      if (hasArrayIndex(jsonPath)) {
        const triggerEl =
          document.activeElement instanceof HTMLElement ? document.activeElement : null;
        setPendingField({ jsonPath, value, triggerEl });
        return;
      }

      // No array index — add directly.
      addSelectedField({
        jsonPath,
        name: fieldNameFromJsonPath(jsonPath),
        type: typeFromValue(value),
        example: exampleFromValue(value),
      });
    },
    [selectedPaths, addSelectedField, removeSelectedField]
  );

  // ── Dialog callbacks ────────────────────────────────────────────────────

  /** "Select for all items" — store the normalised path; type badge = 'array'. */
  const handleNormalize = useCallback(() => {
    if (pendingField === null) return;
    const { jsonPath, value } = pendingField;
    const normalizedPath = normalizeArrayPath(jsonPath);
    addSelectedField({
      jsonPath: normalizedPath,
      name: fieldNameFromJsonPath(normalizedPath),
      type: 'array',
      example: exampleFromValue(value),
    });
    setPendingField(null);
  }, [pendingField, addSelectedField]);

  /** "Select first item only" — store the original indexed path. */
  const handleKeepOriginal = useCallback(() => {
    if (pendingField === null) return;
    const { jsonPath, value } = pendingField;
    addSelectedField({
      jsonPath,
      name: fieldNameFromJsonPath(jsonPath),
      type: typeFromValue(value),
      example: exampleFromValue(value),
    });
    setPendingField(null);
  }, [pendingField, addSelectedField]);

  /** Escape / backdrop click — dismiss without adding a field. */
  const handleDialogClose = useCallback(() => {
    setPendingField(null);
  }, []);

  // ── Render ──────────────────────────────────────────────────────────────

  return (
    <div className="space-y-24">
      <div>
        <h2 className="text-lg font-medium text-text-primary">Select fields</h2>
        <p className="mt-4 text-sm text-text-secondary">
          Click any value in the response to add it as a field your MCP tool will expose.
        </p>
      </div>

      {/* Two-column layout: 3/5 (60%) renderer / 2/5 (40%) panel on xl+; stacked below */}
      <div className="xl:grid xl:grid-cols-5 xl:gap-24 space-y-24 xl:space-y-0">
        {/* Left — Document Renderer — 3 of 5 columns = 60% */}
        <div className="xl:col-span-3 min-w-0">
          <h3
            id="response-heading"
            className="mb-12 text-xs font-medium uppercase tracking-wider text-text-secondary"
          >
            API Response
          </h3>
          <div className="rounded-md border border-border-subtle bg-surface-container-lowest overflow-hidden">
            <DocumentRenderer
              data={responseData}
              selectedPaths={selectedPaths}
              onFieldSelect={handleFieldSelect}
            />
          </div>
        </div>

        {/* Right — Selected fields — 2 of 5 columns = 40% */}
        <div className="xl:col-span-2 min-w-0">
          <SelectedFieldsPanel />
        </div>
      </div>

      {/* Navigation */}
      <div className="flex items-center justify-between pt-8 border-t border-border-subtle">
        <button
          type="button"
          onClick={onBack}
          className="text-sm text-brand-500 hover:underline focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
        >
          &larr; Back
        </button>

        <button
          type="button"
          data-testid="mapping-continue-btn"
          disabled={!canContinue}
          onClick={onContinue}
          title={!canContinue ? 'Select at least one field to continue' : undefined}
          className="rounded-md bg-brand-500 px-24 py-10 text-sm font-medium text-primary-on hover:bg-brand-600 disabled:opacity-40 disabled:cursor-not-allowed focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-2 transition-colors"
        >
          Continue
        </button>
      </div>

      {/* Array normalisation dialog — rendered as a portal-like overlay via showModal() */}
      {pendingField !== null && (
        <ArrayNormalizationDialog
          jsonPath={pendingField.jsonPath}
          responseData={responseData}
          onNormalize={handleNormalize}
          onKeepOriginal={handleKeepOriginal}
          onClose={handleDialogClose}
          triggerEl={pendingField.triggerEl}
        />
      )}
    </div>
  );
}
