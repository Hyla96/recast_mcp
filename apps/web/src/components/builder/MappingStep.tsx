/**
 * MappingStep — step 4 of the builder flow.
 *
 * Renders the API response via DocumentRenderer on the left and the
 * SelectedFieldsPanel on the right (desktop: 60/40 split; tablet/mobile:
 * stacked). Clicking any value in the renderer toggles it in the selected
 * fields list.
 */

import { useCallback, useEffect } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import { DocumentRenderer } from '@components/builder/DocumentRenderer';
import { SelectedFieldsPanel } from '@components/builder/SelectedFieldsPanel';
import { useSelectedPaths } from '@hooks/useSelectedPaths';
import {
  fieldNameFromJsonPath,
  typeFromValue,
  exampleFromValue,
} from '@/lib/fieldMappingUtils';

// ── Props ──────────────────────────────────────────────────────────────────────

interface MappingStepProps {
  onContinue: () => void;
  onBack: () => void;
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

  // Determine the response data to render.
  // sampleJson takes priority when a live test succeeded AND sample was also
  // provided (unlikely, but defensive). If sampleJson is set, use it; otherwise
  // fall back to the live testSlice.response.
  const responseData: unknown = sampleJson !== null ? safeParseJson(sampleJson) : testResponse;

  // Keep stageValidation.mapping in sync with field selection.
  useEffect(() => {
    setStageValid('mapping', selectedFields.length > 0);
  }, [selectedFields.length, setStageValid]);

  const canContinue = selectedFields.length > 0;

  const handleFieldSelect = useCallback(
    (jsonPath: string, value: unknown) => {
      if (selectedPaths.has(jsonPath)) {
        removeSelectedField(jsonPath);
      } else {
        const name = fieldNameFromJsonPath(jsonPath);
        const type = typeFromValue(value);
        const example = exampleFromValue(value);
        addSelectedField({ jsonPath, name, type, example });
      }
    },
    [selectedPaths, addSelectedField, removeSelectedField]
  );

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
    </div>
  );
}
