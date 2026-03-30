import { Link } from 'react-router-dom';

/**
 * DashboardPage — placeholder shell.
 * Full server list with query/polling/status added in TASK-003.
 */
export function DashboardPage() {
  return (
    <div>
      <div className="flex items-center justify-between mb-32">
        <h1 className="text-2xl font-medium text-text-primary">My Servers</h1>
        <Link
          to="/servers/new"
          data-testid="new-server-btn"
          className="px-16 py-8 rounded-md bg-primary text-on-primary text-sm font-medium hover:bg-primary-container transition-colors"
        >
          New server
        </Link>
      </div>

      {/* Empty state — will be replaced by live server grid in TASK-003 */}
      <div className="text-center py-96">
        <div className="mx-auto mb-24 w-80 h-80 rounded-full bg-surface-container-low flex items-center justify-center">
          <ServerIcon className="w-40 h-40 text-text-secondary" />
        </div>
        <h2 className="text-xl font-medium text-text-primary mb-8">No servers yet</h2>
        <p className="text-sm text-text-secondary mb-32">
          Create your first MCP server to start connecting AI agents to your APIs.
        </p>
        <Link
          to="/servers/new"
          data-testid="create-first-server-cta"
          className="inline-block px-24 py-12 rounded-md bg-primary text-on-primary font-medium hover:bg-primary-container transition-colors"
        >
          Create your first server
        </Link>
      </div>
    </div>
  );
}

function ServerIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="2" y="2" width="20" height="8" rx="2" ry="2" />
      <rect x="2" y="14" width="20" height="8" rx="2" ry="2" />
      <line x1="6" y1="6" x2="6.01" y2="6" />
      <line x1="6" y1="18" x2="6.01" y2="18" />
    </svg>
  );
}
