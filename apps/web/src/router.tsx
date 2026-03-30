import { createBrowserRouter } from 'react-router-dom';
import { AppShell } from '@components/layout/AppShell';
import { AuthGuard } from '@components/auth/AuthGuard';
import { HomePage } from '@/pages/HomePage';
import { LoginPage } from '@/pages/LoginPage';
import { DashboardPage } from '@/pages/DashboardPage';
import { NewServerPage } from '@/pages/servers/NewServerPage';
import { ServerDetailPage } from '@/pages/servers/ServerDetailPage';
import { ServerPlaygroundPage } from '@/pages/servers/ServerPlaygroundPage';
import { NotFoundPage } from '@/pages/NotFoundPage';

export const router = createBrowserRouter([
  // Public routes — no app shell
  {
    path: '/',
    element: <HomePage />,
  },
  {
    path: '/login',
    element: <LoginPage />,
  },

  // Authenticated routes — wrapped in AppShell + AuthGuard
  {
    element: (
      <AuthGuard>
        <AppShell />
      </AuthGuard>
    ),
    children: [
      {
        path: '/dashboard',
        element: <DashboardPage />,
      },
      {
        path: '/servers/new',
        element: <NewServerPage />,
      },
      {
        path: '/servers/:id',
        element: <ServerDetailPage />,
      },
      {
        path: '/servers/:id/playground',
        element: <ServerPlaygroundPage />,
      },
    ],
  },

  // 404 catch-all — no shell
  {
    path: '*',
    element: <NotFoundPage />,
  },
]);
