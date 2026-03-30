import React from 'react';
import ReactDOM from 'react-dom/client';
import { ClerkProvider } from '@clerk/clerk-react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from 'react-router-dom';
import { AppErrorBoundary } from '@components/AppErrorBoundary';
import { ToastProvider } from '@/context/ToastContext';
import { router } from '@/router';
import './index.css';

// VITE_CLERK_PUBLISHABLE_KEY must be set in the environment (.env file).
// The build will succeed without it but Clerk components will not render.
const clerkPublishableKey = import.meta.env.VITE_CLERK_PUBLISHABLE_KEY;

if (!clerkPublishableKey) {
  // Warn but do not crash — allows local development without a Clerk account.
  console.warn(
    '[recast-mcp] VITE_CLERK_PUBLISHABLE_KEY is not set. ' +
      'Auth features will be disabled. ' +
      'Copy .env.example → .env and fill in your Clerk publishable key.',
  );
}

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
      gcTime: 300_000,
    },
  },
});

const rootElement = document.getElementById('root');
if (rootElement === null) {
  throw new Error('Root element not found');
}

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <AppErrorBoundary>
      <ClerkProvider
        publishableKey={clerkPublishableKey ?? ''}
        afterSignInUrl="/dashboard"
        afterSignUpUrl="/dashboard"
      >
        <QueryClientProvider client={queryClient}>
          <ToastProvider>
            <RouterProvider router={router} />
          </ToastProvider>
        </QueryClientProvider>
      </ClerkProvider>
    </AppErrorBoundary>
  </React.StrictMode>,
);
