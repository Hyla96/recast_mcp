interface Step {
  id: string;
  label: string;
  completed: boolean;
  active: boolean;
}

interface StepIndicatorProps {
  steps: Step[];
  className?: string;
}

export function StepIndicator({ steps, className = '' }: StepIndicatorProps) {
  return (
    <nav aria-label="Builder steps" className={`flex items-center gap-8 ${className}`}>
      {steps.map((step, index) => (
        <div key={step.id} className="flex items-center gap-8">
          {index > 0 && (
            <div
              className={`h-px w-24 ${
                step.completed ? 'bg-brand-500' : 'bg-border-subtle'
              }`}
            />
          )}
          <div
            className={`flex items-center gap-6 ${step.active ? 'text-text-primary' : 'text-text-secondary'}`}
          >
            <div
              className={`w-24 h-24 rounded-full flex items-center justify-center text-xs font-medium shrink-0 ${
                step.completed
                  ? 'bg-brand-500 text-on-primary'
                  : step.active
                    ? 'border-2 border-brand-500 text-brand-500'
                    : 'border-2 border-border-subtle text-text-secondary'
              }`}
              aria-current={step.active ? 'step' : undefined}
            >
              {step.completed ? '✓' : String(index + 1)}
            </div>
            <span className="text-sm font-medium hidden sm:block">{step.label}</span>
          </div>
        </div>
      ))}
    </nav>
  );
}
