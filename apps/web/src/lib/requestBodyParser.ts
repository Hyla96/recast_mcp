/**
 * Request body template variable extraction.
 *
 * Used by RequestBodyBuilder (TASK-009) and useToolInputSchema (TASK-009).
 */

/**
 * A template variable detected in a request body string, e.g. {{user_id}}.
 *
 * Type is inferred from surrounding JSON context:
 *   'string' - the placeholder is enclosed in double-quotes
 *   'number' - the placeholder appears as a bare (unquoted) JSON value
 */
export interface TemplateVar {
  name: string;
  type: 'string' | 'number';
}

/**
 * A single parameter in the assembled tool input schema.
 * Covers path params, query params, and body template variables.
 */
export interface ToolParameter {
  name: string;
  type: 'string' | 'number' | 'boolean';
  label: string;
  required: boolean;
  source: 'path' | 'query' | 'body';
}

/**
 * Extracts all unique template variables from a request body string.
 *
 * Template variable syntax: {{param_name}} where name matches the pattern
 * [a-zA-Z_][a-zA-Z0-9_]* (letter or underscore, followed by alphanumerics).
 *
 * Type inference: if the placeholder is immediately surrounded by double-quotes
 * (with optional whitespace between the quote and the braces) the type is
 * 'string', otherwise 'number' (bare JSON value).
 *
 * Deduplicates by name; first occurrence wins for type inference.
 * Result order matches first-occurrence order in the body string.
 */
export function extractTemplateVars(body: string): TemplateVar[] {
  const seen = new Map<string, TemplateVar>();
  // Create a fresh regex on each call so lastIndex is always reset.
  const regex = /\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g;

  let match: RegExpExecArray | null;
  while ((match = regex.exec(body)) !== null) {
    const name = match[1];
    if (name === undefined || seen.has(name)) continue;

    // Infer type: look at the characters immediately before and after
    // the placeholder to determine whether it sits inside quotes.
    const before = body.slice(0, match.index);
    const after = body.slice(match.index + match[0].length);
    const isQuoted = /"\s*$/.test(before) && /^\s*"/.test(after);

    seen.set(name, { name, type: isQuoted ? 'string' : 'number' });
  }

  return Array.from(seen.values());
}
