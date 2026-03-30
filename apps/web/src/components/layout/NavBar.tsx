import { Link } from 'react-router-dom';
import { useUiStore } from '@stores/uiStore';

export function NavBar() {
  const { theme, toggleTheme } = useUiStore();

  return (
    <header className="fixed top-0 inset-x-0 z-fixed h-48 bg-surface-container-low flex items-center px-24">
      {/* Logo */}
      <Link
        to="/dashboard"
        className="flex items-center gap-8 font-semibold text-text-primary hover:text-primary transition-colors mr-auto"
        aria-label="Recast MCP — go to dashboard"
      >
        <RecastLogo className="w-24 h-24" />
        <span className="hidden sm:block">Recast MCP</span>
      </Link>

      {/* Right side controls */}
      <div className="flex items-center gap-8">
        {/* Theme toggle */}
        <button
          type="button"
          onClick={toggleTheme}
          aria-label={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
          aria-pressed={theme === 'dark'}
          className="p-8 rounded-md text-text-secondary hover:text-text-primary hover:bg-surface-container-highest transition-colors"
        >
          {theme === 'dark' ? (
            <SunIcon className="w-16 h-16" />
          ) : (
            <MoonIcon className="w-16 h-16" />
          )}
        </button>

        {/* User menu placeholder — replaced by Clerk UserButton in TASK-002 */}
        <div className="w-32 h-32 rounded-full bg-surface-container-highest flex items-center justify-center text-sm font-medium text-text-secondary">
          U
        </div>
      </div>
    </header>
  );
}

function RecastLogo({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
    >
      <rect width="24" height="24" rx="6" fill="currentColor" className="text-primary" />
      <path
        d="M7 8h10M7 12h6M7 16h8"
        stroke="white"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

function SunIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="5" />
      <line x1="12" y1="1" x2="12" y2="3" />
      <line x1="12" y1="21" x2="12" y2="23" />
      <line x1="4.22" y1="4.22" x2="5.64" y2="5.64" />
      <line x1="18.36" y1="18.36" x2="19.78" y2="19.78" />
      <line x1="1" y1="12" x2="3" y2="12" />
      <line x1="21" y1="12" x2="23" y2="12" />
      <line x1="4.22" y1="19.78" x2="5.64" y2="18.36" />
      <line x1="18.36" y1="5.64" x2="19.78" y2="4.22" />
    </svg>
  );
}

function MoonIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z" />
    </svg>
  );
}
