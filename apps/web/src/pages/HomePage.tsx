import { Link } from 'react-router-dom';

export function HomePage() {
  return (
    <div className="min-h-screen bg-surface flex flex-col items-center justify-center px-16">
      <h1 className="text-3xl font-medium text-text-primary mb-16">Recast MCP</h1>
      <p className="text-text-secondary mb-32">
        Expose any REST API to AI agents as a live MCP server.
      </p>
      <div className="flex gap-16">
        <Link
          to="/dashboard"
          className="px-24 py-12 rounded-md bg-primary text-on-primary font-medium hover:bg-primary-container transition-colors"
        >
          Go to dashboard
        </Link>
        <Link
          to="/login"
          className="px-24 py-12 rounded-md text-text-secondary hover:text-text-primary transition-colors"
        >
          Sign in
        </Link>
      </div>
    </div>
  );
}
