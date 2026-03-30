import type React from 'react';
import { StepErrorBoundary } from '../StepErrorBoundary';

interface StepLayoutProps {
  title: string;
  description?: string;
  children: React.ReactNode;
  stepName?: string;
  className?: string;
}

export function StepLayout({
  title,
  description,
  children,
  stepName,
  className = '',
}: StepLayoutProps) {
  return (
    <StepErrorBoundary stepName={stepName ?? title}>
      <section className={`space-y-24 ${className}`}>
        <div>
          <h2 className="text-xl font-medium text-text-primary">{title}</h2>
          {description !== undefined && (
            <p className="mt-8 text-sm text-text-secondary">{description}</p>
          )}
        </div>
        {children}
      </section>
    </StepErrorBoundary>
  );
}
