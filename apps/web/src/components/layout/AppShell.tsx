import { Outlet, ScrollRestoration } from 'react-router-dom';
import { NavBar } from './NavBar';

export function AppShell() {
  return (
    <div className="min-h-screen bg-surface">
      <NavBar />
      {/* Main content area with top padding to clear fixed navbar */}
      <main className="pt-48">
        <div className="mx-auto max-w-screen-xl px-16 py-32 sm:px-24 lg:px-32">
          <Outlet />
        </div>
      </main>
      <ScrollRestoration />
    </div>
  );
}
