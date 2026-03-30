import { useParams } from 'react-router-dom';
import { Link } from 'react-router-dom';

/**
 * ServerDetailPage — placeholder shell.
 * Full server detail and configuration view added in a future task.
 */
export function ServerDetailPage() {
  const { id } = useParams<{ id: string }>();

  return (
    <div>
      <h1 className="text-2xl font-medium text-text-primary mb-32">
        Server: {id ?? 'unknown'}
      </h1>
      <div className="flex gap-16 mb-32">
        <Link
          to={`/servers/${id ?? ''}/playground`}
          className="px-16 py-8 rounded-md bg-primary text-on-primary text-sm font-medium hover:bg-primary-container transition-colors"
        >
          Open playground
        </Link>
        <Link
          to="/dashboard"
          className="px-16 py-8 rounded-md text-text-secondary text-sm hover:text-text-primary transition-colors"
        >
          Back to dashboard
        </Link>
      </div>
      <div className="rounded-md bg-surface-container-low p-32 text-center text-sm text-text-secondary">
        Server detail view coming in a future task
      </div>
    </div>
  );
}
