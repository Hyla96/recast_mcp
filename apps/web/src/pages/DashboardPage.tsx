import { useMemo } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useUser } from '@clerk/clerk-react';
import { ServerCard, SkeletonCard } from '@components/dashboard/ServerCard';
import { useServerStatusSocket } from '@hooks/useServerStatusSocket';
import { useFetchWithAuth } from '@/lib/fetchWithAuth';
import type { McpServer, ServersListResponse } from '@/types/server';

// ─── Server List Query ────────────────────────────────────────────────────────

function useServers(userId: string | null | undefined) {
  const fetchAuth = useFetchWithAuth();

  return useQuery<ServersListResponse>({
    queryKey: ['servers', userId],
    queryFn: async () => {
      const res = await fetchAuth('/api/v1/servers');
      if (!res.ok) {
        throw new Error(`Failed to fetch servers: ${res.status}`);
      }
      return res.json() as Promise<ServersListResponse>;
    },
    enabled: Boolean(userId),
    refetchInterval: 30_000,
  });
}

// ─── Sub-components ───────────────────────────────────────────────────────────

function LoadingGrid() {
  return (
    <div
      className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-24"
      aria-label="Loading servers"
    >
      <SkeletonCard />
      <SkeletonCard />
      <SkeletonCard />
    </div>
  );
}

interface ErrorStateProps {
  onRetry: () => void;
}

function ErrorState({ onRetry }: ErrorStateProps) {
  return (
    <div className="rounded-lg border border-error-container bg-surface-container-lowest p-32 text-center">
      <p className="text-base font-medium text-text-primary mb-8">
        Failed to load servers
      </p>
      <p className="text-sm text-text-secondary mb-24">
        Something went wrong while fetching your servers.
      </p>
      <button
        type="button"
        onClick={onRetry}
        className="px-16 py-8 rounded-md bg-primary text-primary-on text-sm font-medium hover:bg-primary-container transition-colors"
      >
        Try again
      </button>
    </div>
  );
}

function EmptyState() {
  return (
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
        className="inline-block px-24 py-12 rounded-md bg-primary text-primary-on font-medium hover:bg-primary-container transition-colors"
      >
        Create your first server
      </Link>
    </div>
  );
}

interface ServerGridProps {
  servers: McpServer[];
}

function ServerGrid({ servers }: ServerGridProps) {
  // Sort by updatedAt descending
  const sorted = useMemo<McpServer[]>(
    () =>
      [...servers].sort(
        (a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime(),
      ),
    [servers],
  );

  if (sorted.length === 0) {
    return <EmptyState />;
  }

  return (
    <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-24">
      {sorted.map((server) => (
        <ServerCard key={server.id} server={server} />
      ))}
    </div>
  );
}

// ─── Dashboard Page ───────────────────────────────────────────────────────────

export function DashboardPage() {
  const { user, isLoaded } = useUser();
  const userId = isLoaded ? (user?.id ?? null) : undefined;

  const { data, isLoading, isError, refetch } = useServers(userId);

  // Real-time status updates via WebSocket; falls back to polling silently
  useServerStatusSocket(userId ?? null);

  return (
    <div>
      {/* Page header */}
      <div className="flex items-center justify-between mb-32">
        <h1 className="text-2xl font-medium text-text-primary">My Servers</h1>
        <Link
          to="/servers/new"
          data-testid="new-server-btn"
          className="px-16 py-8 rounded-md bg-primary text-primary-on text-sm font-medium hover:bg-primary-container transition-colors"
        >
          New server
        </Link>
      </div>

      {/* Content */}
      {isLoading || !isLoaded ? (
        <LoadingGrid />
      ) : isError ? (
        <ErrorState onRetry={() => void refetch()} />
      ) : (
        <ServerGrid servers={data?.data ?? []} />
      )}
    </div>
  );
}

// ─── Icons ────────────────────────────────────────────────────────────────────

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
