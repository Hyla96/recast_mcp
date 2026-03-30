import { useState } from 'react';

interface EncryptedFieldBadgeProps {
  className?: string;
}

export function EncryptedFieldBadge({ className = '' }: EncryptedFieldBadgeProps) {
  const [tooltipVisible, setTooltipVisible] = useState(false);

  return (
    <div className={`relative inline-flex items-center ${className}`}>
      <button
        type="button"
        aria-label="This field is encrypted at rest"
        onMouseEnter={() => setTooltipVisible(true)}
        onMouseLeave={() => setTooltipVisible(false)}
        onFocus={() => setTooltipVisible(true)}
        onBlur={() => setTooltipVisible(false)}
        className="flex items-center gap-4 rounded-xs bg-tertiary-container px-6 py-3 text-xs font-medium text-on-surface-variant"
      >
        <LockIcon className="w-10 h-10" />
        <span>Encrypted</span>
      </button>
      {tooltipVisible && (
        <div
          role="tooltip"
          className="absolute bottom-full left-1/2 -translate-x-1/2 mb-4 w-48 rounded-md bg-inverse-surface px-12 py-8 text-xs text-inverse-on-surface shadow-float z-dropdown"
        >
          This value is encrypted with AES-256-GCM and never stored in plain text.
          <div className="absolute top-full left-1/2 -translate-x-1/2 border-4 border-transparent border-t-inverse-surface" />
        </div>
      )}
    </div>
  );
}

function LockIcon({ className }: { className?: string }) {
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
      <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
      <path d="M7 11V7a5 5 0 0110 0v4" />
    </svg>
  );
}
