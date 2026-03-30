/**
 * AuthGuard — protected layout wrapper.
 *
 * Renders its children only when the user is signed in. Unauthenticated users
 * are redirected to `/login?redirect_url=<current path>` so that after sign-in
 * they are bounced back to their original destination.
 */

import { useAuth } from '@clerk/clerk-react';
import { Navigate, useLocation } from 'react-router-dom';

interface AuthGuardProps {
  children: React.ReactNode;
}

export function AuthGuard({ children }: AuthGuardProps) {
  const { isLoaded, isSignedIn } = useAuth();
  const location = useLocation();

  // Wait until Clerk has finished resolving the session.
  if (!isLoaded) {
    return (
      <div className="min-h-screen bg-surface flex items-center justify-center">
        <span className="text-text-secondary text-sm" role="status">
          Loading…
        </span>
      </div>
    );
  }

  // Redirect unauthenticated users to /login preserving the intended URL.
  if (!isSignedIn) {
    const redirectUrl = encodeURIComponent(location.pathname + location.search);
    return <Navigate to={`/login?redirect_url=${redirectUrl}`} replace />;
  }

  return <>{children}</>;
}
