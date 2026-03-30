import { useState, useEffect } from 'react';

/**
 * Returns a debounced copy of `value` that updates only after `delay` ms of
 * inactivity. Cleans up the timer automatically on unmount.
 */
export function useDebounce<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState<T>(value);

  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(id);
  }, [value, delay]);

  return debounced;
}
