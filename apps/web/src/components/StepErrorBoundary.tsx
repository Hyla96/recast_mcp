import React from 'react';

interface Props {
  children: React.ReactNode;
  stepName?: string;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export class StepErrorBoundary extends React.Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: unknown): State {
    return {
      hasError: true,
      error: error instanceof Error ? error : new Error(String(error)),
    };
  }

  componentDidCatch(error: unknown, info: React.ErrorInfo): void {
    console.error('StepErrorBoundary caught in step', this.props.stepName, error, info);
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="rounded-md bg-error-container p-16">
          <p className="text-sm text-error font-medium">
            {this.props.stepName !== undefined
              ? `Error in step "${this.props.stepName}"`
              : 'Error in builder step'}
          </p>
          <p className="text-xs text-text-secondary mt-4">
            {this.state.error?.message ?? 'An unexpected error occurred.'}
          </p>
          <button
            type="button"
            onClick={() => this.setState({ hasError: false, error: null })}
            className="mt-8 text-xs text-error underline"
          >
            Try again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
