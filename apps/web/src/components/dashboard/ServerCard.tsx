import { Link } from 'react-router-dom';
import type { McpServer, ServerStatus } from '@/types/server';

// ─── Relative Time Formatter ─────────────────────────────────────────────────

const rtf = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });

function formatRelativeTime(isoDate: string | null): string {
  if (!isoDate) return 'Never';

  const diffMs = new Date(isoDate).getTime() - Date.now();
  const diffSec = diffMs / 1000;
  const absSec = Math.abs(diffSec);

  if (absSec < 60) return rtf.format(Math.round(diffSec), 'second');
  if (absSec < 3600) return rtf.format(Math.round(diffSec / 60), 'minute');
  if (absSec < 86_400) return rtf.format(Math.round(diffSec / 3600), 'hour');
  if (absSec < 86_400 * 30) return rtf.format(Math.round(diffSec / 86_400), 'day');
  if (absSec < 86_400 * 365) return rtf.format(Math.round(diffSec / (86_400 * 30)), 'month');
  return rtf.format(Math.round(diffSec / (86_400 * 365)), 'year');
}

// ─── Call Count Formatter ─────────────────────────────────────────────────────

const nf = new Intl.NumberFormat();

function formatCallCount(count: number): string {
  return nf.format(count);
}

// ─── Status Badge ─────────────────────────────────────────────────────────────

const statusConfig: Record<
  ServerStatus,
  { label: string; classes: string }
> = {
  active: {
    label: 'Active',
    classes: 'bg-secondary-container text-secondary',
  },
  error: {
    label: 'Error',
    classes: 'bg-error-container text-error',
  },
  inactive: {
    label: 'Inactive',
    classes: 'bg-surface-variant text-text-secondary',
  },
};

function StatusBadge({ status }: { status: ServerStatus }) {
  const config = statusConfig[status];
  return (
    <span
      className={`inline-flex items-center px-8 py-2 rounded-full text-xs font-medium ${config.classes}`}
    >
      {config.label}
    </span>
  );
}

// ─── Server Card ──────────────────────────────────────────────────────────────

interface ServerCardProps {
  server: McpServer;
}

/**
 * A single MCP server card. The entire card is a single focusable Link element
 * so keyboard users can tab to it and activate with Enter.
 */
export function ServerCard({ server }: ServerCardProps) {
  return (
    <Link
      to={`/servers/${server.id}`}
      aria-label={`${server.name}, ${statusConfig[server.status].label}`}
      className={[
        'block rounded-lg border border-border-subtle bg-surface-container-lowest',
        'p-24 shadow-none transition-shadow duration-normal ease-standard',
        'hover:shadow-float hover:border-border-default',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500',
      ].join(' ')}
    >
      {/* Header: name + status */}
      <div className="flex items-start justify-between gap-16 mb-16">
        <span className="text-base font-semibold text-text-primary truncate">
          {server.name}
        </span>
        <div className="flex items-center gap-8 shrink-0">
          {server.isUnverified === true && (
            <span
              title="Built with a manually pasted sample response — not live-tested against the API"
              className="inline-flex items-center px-8 py-2 rounded-full text-xs font-medium bg-amber-100 text-amber-800 dark:bg-amber-900 dark:text-amber-200"
            >
              Sample response
            </span>
          )}
          <StatusBadge status={server.status} />
        </div>
      </div>

      {/* Endpoint URL */}
      <p
        className="text-xs font-mono text-text-secondary truncate mb-20"
        title={server.endpointUrl}
      >
        {server.endpointUrl}
      </p>

      {/* Footer: last call + call count */}
      <div className="flex items-center justify-between text-xs text-text-secondary">
        <span>
          <span className="text-text-primary font-medium">Last call:</span>{' '}
          {formatRelativeTime(server.lastCallAt)}
        </span>
        <span>
          <span className="text-text-primary font-medium">
            {formatCallCount(server.callCount24h)}
          </span>{' '}
          calls (24h)
        </span>
      </div>
    </Link>
  );
}

// ─── Skeleton Card ────────────────────────────────────────────────────────────

/**
 * Animated placeholder shown during the initial server list fetch.
 * Uses `animate-pulse` per spec — no spinner.
 */
export function SkeletonCard() {
  return (
    <div
      className="rounded-lg border border-border-subtle bg-surface-container-lowest p-24 animate-pulse"
      aria-hidden="true"
    >
      <div className="flex items-start justify-between gap-16 mb-16">
        <div className="h-16 bg-surface-container-high rounded w-2/3" />
        <div className="h-16 bg-surface-container-high rounded-full w-16" />
      </div>
      <div className="h-12 bg-surface-container-high rounded w-full mb-20" />
      <div className="flex items-center justify-between">
        <div className="h-12 bg-surface-container-high rounded w-32" />
        <div className="h-12 bg-surface-container-high rounded w-24" />
      </div>
    </div>
  );
}
