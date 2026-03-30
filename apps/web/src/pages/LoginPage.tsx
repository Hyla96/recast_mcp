import { Link } from 'react-router-dom';

/**
 * LoginPage — placeholder shell.
 * Clerk <SignIn> / <SignUp> integration added in TASK-002.
 */
export function LoginPage() {
  return (
    <div className="min-h-screen bg-surface flex items-center justify-center px-16">
      <div className="w-full max-w-sm">
        <h1 className="text-2xl font-medium text-text-primary mb-32 text-center">
          Sign in to Recast MCP
        </h1>
        {/* Clerk SignIn component will be rendered here (TASK-002) */}
        <div className="rounded-md bg-surface-container-low p-32 text-center text-sm text-text-secondary">
          Authentication coming in TASK-002
        </div>
        <p className="mt-24 text-center text-sm text-text-secondary">
          Don't have an account?{' '}
          <Link to="/login?mode=signup" className="text-secondary underline">
            Create one
          </Link>
        </p>
        <p className="mt-16 text-center text-sm">
          <Link to="/dashboard" className="text-text-secondary underline">
            Continue to dashboard
          </Link>
        </p>
      </div>
    </div>
  );
}
