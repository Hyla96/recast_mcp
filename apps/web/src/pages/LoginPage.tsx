/**
 * LoginPage — Clerk-powered sign-in / sign-up.
 *
 * URL query param `mode`:
 *   - absent or "signin" → renders <SignIn>
 *   - "signup"           → renders <SignUp>
 *
 * Already-authenticated users are immediately redirected to /dashboard.
 */

import { SignIn, SignUp, useAuth } from '@clerk/clerk-react';
import { useEffect } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { useUiStore } from '@stores/uiStore';
import { buildClerkAppearance } from '@/lib/clerkAppearance';

export function LoginPage() {
  const { isLoaded, isSignedIn } = useAuth();
  const { theme } = useUiStore();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  const mode = searchParams.get('mode');
  const showSignUp = mode === 'signup';
  // Preserve intended destination for post-auth redirect.
  const redirectUrl = searchParams.get('redirect_url') ?? '/dashboard';

  // Redirect already-authenticated users immediately.
  useEffect(() => {
    if (isLoaded && isSignedIn) {
      void navigate(redirectUrl, { replace: true });
    }
  }, [isLoaded, isSignedIn, navigate, redirectUrl]);

  const appearance = buildClerkAppearance(theme);

  return (
    <div className="min-h-screen bg-surface flex items-center justify-center px-16 py-32">
      <div className="w-full max-w-sm">
        {showSignUp ? (
          <SignUp
            appearance={appearance}
            redirectUrl={redirectUrl}
            signInUrl="/login"
          />
        ) : (
          <SignIn
            appearance={appearance}
            redirectUrl={redirectUrl}
            signUpUrl="/login?mode=signup"
          />
        )}
      </div>
    </div>
  );
}
