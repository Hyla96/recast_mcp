import { useParams, Link } from 'react-router-dom';

/**
 * ServerPlaygroundPage — placeholder shell.
 * Full MCP playground UI added in a future task.
 */
export function ServerPlaygroundPage() {
  const { id } = useParams<{ id: string }>();

  return (
    <div>
      <div className="flex items-center gap-16 mb-32">
        <Link
          to={`/servers/${id ?? ''}`}
          className="text-sm text-text-secondary hover:text-text-primary transition-colors"
        >
          ← Back to server
        </Link>
        <h1 className="text-2xl font-medium text-text-primary">
          Playground — {id ?? 'unknown'}
        </h1>
      </div>
      <div className="rounded-md bg-surface-container-low p-32 text-center text-sm text-text-secondary">
        Playground UI coming in a future task
      </div>
    </div>
  );
}
