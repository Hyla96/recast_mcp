import { useMemo, useRef } from 'react';
import { useBuilderStore } from '@stores/builderStore';

/**
 * Returns a stable Set<string> of selected JSONPaths from the mapping slice.
 *
 * Uses a useRef-based memoization layer on top of useMemo: when the set of
 * selected paths has not changed (same size, same members), the previous Set
 * reference is returned. This prevents unnecessary re-renders in components
 * that receive the set as a prop.
 */
export function useSelectedPaths(): Set<string> {
  const selectedFields = useBuilderStore((s) => s.mappingSlice.selectedFields);
  const setRef = useRef<Set<string>>(new Set<string>());

  return useMemo(() => {
    const next = new Set(selectedFields.map((f) => f.jsonPath));
    const prev = setRef.current;

    if (prev.size === next.size && [...next].every((p) => prev.has(p))) {
      return prev;
    }

    setRef.current = next;
    return next;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedFields]);
}
