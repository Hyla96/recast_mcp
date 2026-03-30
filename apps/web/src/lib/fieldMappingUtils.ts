/**
 * Pure utility functions for field mapping.
 *
 * Extracts human-friendly names and type labels from JSONPaths and raw values.
 * All functions are side-effect-free.
 */

// ISO 8601 date or datetime string detection.
const ISO_DATE_RE =
  /^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$/;

/**
 * Extracts a human-friendly field name from a JSONPath string.
 *
 * Examples:
 *   $.name            -> "name"
 *   $.user.firstName  -> "firstName"
 *   $.items[0].price  -> "price"
 *   $.items[*].price  -> "price"
 *   $['foo bar']      -> "foo bar"
 *   $                 -> "field"
 */
export function fieldNameFromJsonPath(jsonPath: string): string {
  // Last dot-notation segment, optionally followed by array index(es).
  const dotMatch = jsonPath.match(/\.([a-zA-Z_$][a-zA-Z0-9_$]*)(?:\[[\d*]+\])*\s*$/);
  if (dotMatch?.[1]) return dotMatch[1];

  // Bracket notation string key (e.g. $['foo bar'][0]).
  const bracketStringMatch = jsonPath.match(/\['([^']+)'\](?:\[[\d*]+\])*\s*$/);
  if (bracketStringMatch?.[1]) return bracketStringMatch[1];

  return 'field';
}

/**
 * Returns a simplified type label for use in the field type badge.
 */
export function typeFromValue(value: unknown): string {
  if (value === null || value === undefined) return 'string';
  if (typeof value === 'boolean') return 'boolean';
  if (typeof value === 'number') return 'number';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'object') return 'object';
  if (typeof value === 'string') {
    if (ISO_DATE_RE.test(value)) {
      const d = new Date(value);
      if (!isNaN(d.getTime())) return 'date';
    }
    return 'string';
  }
  return 'string';
}

/**
 * Converts a raw value to a short example display string (max 40 chars).
 */
export function exampleFromValue(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'number') return String(value);
  if (typeof value === 'string') {
    return value.length > 40 ? value.slice(0, 37) + '...' : value;
  }
  try {
    const s = JSON.stringify(value);
    return s.length > 40 ? s.slice(0, 37) + '...' : s;
  } catch {
    return '[complex]';
  }
}

/**
 * Filters a raw input string to only allow characters valid in field names:
 * letters, digits, spaces, hyphens, and underscores. Max 64 characters.
 */
export function filterFieldNameInput(value: string): string {
  return value.replace(/[^a-zA-Z0-9 _-]/g, '').slice(0, 64);
}
