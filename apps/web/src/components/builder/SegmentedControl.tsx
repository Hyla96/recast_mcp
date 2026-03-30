import type React from 'react';

interface Option<T extends string> {
  value: T;
  label: string;
}

interface SegmentedControlProps<T extends string> {
  options: Option<T>[];
  value: T;
  onChange: (value: T) => void;
  name: string;
  'aria-label'?: string;
  className?: string;
}

export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  name,
  'aria-label': ariaLabel,
  className = '',
}: SegmentedControlProps<T>) {
  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const currentIndex = options.findIndex((o) => o.value === value);
    let nextIndex = currentIndex;

    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
      e.preventDefault();
      nextIndex = (currentIndex + 1) % options.length;
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
      e.preventDefault();
      nextIndex = (currentIndex - 1 + options.length) % options.length;
    }

    const next = options[nextIndex];
    if (next !== undefined && nextIndex !== currentIndex) {
      onChange(next.value);
    }
  };

  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      onKeyDown={handleKeyDown}
      className={`inline-flex rounded-md bg-surface-container p-2 gap-2 ${className}`}
    >
      {options.map((option) => {
        const isSelected = option.value === value;
        return (
          <label
            key={option.value}
            className={`relative flex cursor-pointer select-none items-center rounded-sm px-12 py-6 text-sm font-medium transition-colors ${
              isSelected
                ? 'bg-surface-container-lowest text-text-primary shadow-float'
                : 'text-text-secondary hover:text-text-primary'
            }`}
          >
            <input
              type="radio"
              name={name}
              value={option.value}
              checked={isSelected}
              onChange={() => onChange(option.value)}
              className="sr-only"
              tabIndex={isSelected ? 0 : -1}
            />
            {option.label}
          </label>
        );
      })}
    </div>
  );
}
