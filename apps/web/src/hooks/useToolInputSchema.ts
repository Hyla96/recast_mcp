import { useMemo } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import { extractTemplateVars } from '@/lib/requestBodyParser';
import type { ToolParameter } from '@/lib/requestBodyParser';

export type { ToolParameter };

/**
 * Assembles the complete tool input schema from the builder store:
 *  - URL path parameters  (source: 'path',  always required)
 *  - URL query parameters (source: 'query', optional by default)
 *  - Request body template variables — POST/PUT/PATCH only
 *    (source: 'body', always required)
 *
 * Template variables are derived via `useMemo` from
 * `requestBodySlice.requestBody` and are never stored in Zustand.
 *
 * Returns a flat `ToolParameter[]` in the order:
 *   path params → query params → body template vars.
 */
export function useToolInputSchema(): ToolParameter[] {
  const pathParams = useBuilderStore((s) => s.urlSlice.pathParams);
  const queryParams = useBuilderStore((s) => s.urlSlice.queryParams);
  const requestBody = useBuilderStore((s) => s.requestBodySlice.requestBody);

  return useMemo((): ToolParameter[] => {
    const pathTools: ToolParameter[] = pathParams.map((p) => ({
      name: p.name,
      type: p.type,
      label: p.name,
      required: true,
      source: 'path',
    }));

    const queryTools: ToolParameter[] = queryParams.map((q) => ({
      name: q.key,
      type: q.type,
      label: q.key,
      required: false,
      source: 'query',
    }));

    const bodyTools: ToolParameter[] =
      requestBody !== null
        ? extractTemplateVars(requestBody).map((v) => ({
            name: v.name,
            type: v.type === 'number' ? ('number' as const) : ('string' as const),
            label: v.name,
            required: true,
            source: 'body' as const,
          }))
        : [];

    return [...pathTools, ...queryTools, ...bodyTools];
  }, [pathParams, queryParams, requestBody]);
}
