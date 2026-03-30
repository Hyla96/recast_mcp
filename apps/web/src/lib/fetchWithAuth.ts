/**
 * Authenticated fetch utility.
 *
 * Wraps `fetch` with Clerk JWT injection. On 401 responses it calls
 * `signOut()` and redirects to `/login`.
 *
 * Usage (inside a React component or hook):
 *
 *   const { getToken, signOut } = useAuth();
 *   const fetcher = buildFetchWithAuth(getToken, signOut);
 *   const data = await fetcher('/api/v1/servers');
 *
 * The returned function mirrors the native `fetch` signature.
 */

import { useAuth } from '@clerk/clerk-react';

type GetToken = () => Promise<string | null>;
type SignOutFn = () => Promise<unknown>;

export function buildFetchWithAuth(
  getToken: GetToken,
  signOut: SignOutFn,
  redirectPath = '/login',
) {
  return async function fetchWithAuth(
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> {
    const token = await getToken();

    const headers = new Headers(init?.headers);
    if (token !== null) {
      headers.set('Authorization', `Bearer ${token}`);
    }

    const response = await fetch(input, { ...init, headers });

    if (response.status === 401) {
      await signOut();
      window.location.href = redirectPath;
      // The redirect above navigates away immediately; returning here satisfies
      // TypeScript's return type requirement.
      return response;
    }

    return response;
  };
}

/**
 * React hook returning a `fetchWithAuth` function bound to the current Clerk
 * session. Must be called inside a component tree wrapped by `<ClerkProvider>`.
 *
 * Example:
 *   const fetchAuth = useFetchWithAuth();
 *   const res = await fetchAuth('/api/v1/servers');
 */
export function useFetchWithAuth() {
  const { getToken, signOut } = useAuth();
  return buildFetchWithAuth(getToken, signOut);
}
