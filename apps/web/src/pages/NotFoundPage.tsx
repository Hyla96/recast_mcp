import { Link } from 'react-router-dom';

export function NotFoundPage() {
  return (
    <div className="min-h-screen bg-surface flex items-center justify-center px-16">
      <div className="text-center">
        <p className="text-sm font-medium text-text-secondary uppercase tracking-widest mb-16">
          404
        </p>
        <h1 className="text-3xl font-medium text-text-primary mb-16">Page not found</h1>
        <p className="text-text-secondary mb-32">
          The page you're looking for doesn't exist or has been moved.
        </p>
        <Link
          to="/dashboard"
          className="inline-block px-24 py-12 rounded-md bg-primary text-on-primary font-medium hover:bg-primary-container transition-colors"
        >
          Go to dashboard
        </Link>
      </div>
    </div>
  );
}
