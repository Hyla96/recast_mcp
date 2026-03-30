/**
 * NewServerPage — builder flow root.
 *
 * Manages the top-level stage progression and renders the active step.
 * Step components are self-contained and read/write Zustand builderStore
 * directly; this page only wires the Continue/Back navigation.
 */

import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useBuilderStore } from '@stores/builderStore';
import type { BuilderStage } from '@stores/builderStore';
import { StepIndicator } from '@components/builder/StepIndicator';
import { UrlStep } from '@components/builder/UrlStep';

// ── Step metadata ─────────────────────────────────────────────────────────────

const STEPS: Array<{ id: BuilderStage; label: string }> = [
  { id: 'url', label: 'Endpoint' },
  { id: 'auth', label: 'Auth' },
  { id: 'test', label: 'Test' },
  { id: 'mapping', label: 'Fields' },
  { id: 'naming', label: 'Name' },
  { id: 'review', label: 'Review' },
];

// ── Page ──────────────────────────────────────────────────────────────────────

export function NewServerPage() {
  const navigate = useNavigate();
  const currentStage = useBuilderStore((s) => s.currentStage);
  const stageValidation = useBuilderStore((s) => s.stageValidation);
  const setCurrentStage = useBuilderStore((s) => s.setCurrentStage);
  const resetBuilder = useBuilderStore((s) => s.resetBuilder);

  // Reset builder state when the user navigates fresh to /servers/new.
  useEffect(() => {
    resetBuilder();
    // Only run on mount — resetting on every render would clear state on
    // in-step navigation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const currentIndex = STEPS.findIndex((s) => s.id === currentStage);

  const stepsForIndicator = STEPS.map((step, i) => ({
    id: step.id,
    label: step.label,
    completed: i < currentIndex,
    active: i === currentIndex,
  }));

  const handleContinue = () => {
    const next = STEPS[currentIndex + 1];
    if (next !== undefined) {
      setCurrentStage(next.id);
    } else {
      // Last step — navigate to dashboard after review/submit.
      navigate('/dashboard');
    }
  };

  const renderStep = () => {
    switch (currentStage) {
      case 'url':
        return <UrlStep onContinue={handleContinue} />;

      case 'auth':
      case 'test':
      case 'mapping':
      case 'naming':
      case 'review':
        return (
          <div className="rounded-md bg-surface-container-low p-32 text-center text-sm text-text-secondary">
            Step: <strong>{currentStage}</strong> — coming in a future task
            <div className="mt-16 flex justify-between">
              <button
                type="button"
                onClick={() => {
                  const prev = STEPS[currentIndex - 1];
                  if (prev !== undefined) setCurrentStage(prev.id);
                }}
                className="text-sm text-brand-500 hover:underline"
              >
                ← Back
              </button>
              <button
                type="button"
                disabled={!stageValidation[currentStage]}
                onClick={handleContinue}
                className="rounded-md bg-brand-500 px-24 py-10 text-sm font-medium text-primary-on hover:bg-brand-600 disabled:opacity-40 disabled:cursor-not-allowed"
              >
                Continue
              </button>
            </div>
          </div>
        );
    }
  };

  return (
    <div className="max-w-2xl mx-auto space-y-32">
      <div>
        <h1 className="text-2xl font-medium text-text-primary">New MCP Server</h1>
        <p className="mt-8 text-sm text-text-secondary">
          Connect a REST API to Claude in a few steps.
        </p>
      </div>

      <StepIndicator steps={stepsForIndicator} />

      <div className="rounded-xl bg-surface-container-lowest shadow-float p-32">
        {renderStep()}
      </div>
    </div>
  );
}
