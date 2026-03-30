/**
 * Utilities for tool name generation and validation.
 *
 * Tool names must match: `[a-z0-9_]+`, length 3–50, no leading/trailing
 * underscores, no consecutive underscores.
 */

// ─── Generation ──────────────────────────────────────────────────────────────

/**
 * Derives a suggested tool name from a REST API URL.
 *
 * Algorithm:
 * 1. Parse URL; extract decoded pathname.
 * 2. Split on `/`, discard segments that are path-param placeholders
 *    (`{name}` form) or empty.
 * 3. Join remaining segments with `_`.
 * 4. Lowercase, replace any character outside `[a-z0-9_]` with `_`.
 * 5. Collapse consecutive underscores and strip leading/trailing underscores.
 * 6. Truncate to 50 characters.
 * 7. If the result is fewer than 3 characters, return `'my_tool'`.
 *
 * Returns `'my_tool'` for invalid or empty URLs.
 */
export function generateToolName(url: string): string {
  if (!url.trim()) return 'my_tool';

  let pathname: string;
  try {
    pathname = decodeURIComponent(new URL(url).pathname);
  } catch {
    return 'my_tool';
  }

  const segments = pathname
    .split('/')
    .filter((seg) => seg.length > 0 && !/^\{.*\}$/.test(seg));

  if (segments.length === 0) return 'my_tool';

  const raw = segments.join('_');
  const cleaned = raw
    .toLowerCase()
    .replace(/[^a-z0-9_]/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 50);

  return cleaned.length >= 3 ? cleaned : 'my_tool';
}

// ─── Validation ──────────────────────────────────────────────────────────────

/**
 * Validates a tool name against all naming rules.
 *
 * Returns a human-readable error string for the first rule that fails,
 * or `null` when the name is valid.
 *
 * Rules (checked in order):
 * 1. Must be at least 3 characters.
 * 2. Must be at most 50 characters.
 * 3. Must contain only lowercase letters, digits, and underscores.
 * 4. Must not start or end with an underscore.
 * 5. Must not contain consecutive underscores.
 */
export function validateToolName(name: string): string | null {
  if (name.length < 3) {
    return 'Tool name must be at least 3 characters.';
  }
  if (name.length > 50) {
    return 'Tool name must be at most 50 characters.';
  }
  if (!/^[a-z0-9_]+$/.test(name)) {
    return 'Tool name may only contain lowercase letters, digits, and underscores.';
  }
  if (name.startsWith('_') || name.endsWith('_')) {
    return 'Tool name must not start or end with an underscore.';
  }
  if (/__/.test(name)) {
    return 'Tool name must not contain consecutive underscores.';
  }
  return null;
}

/**
 * Filters a raw keystroke string to only allowed characters: `[a-z0-9_]`.
 * Use on the `onChange` handler of the tool name input to silently reject
 * invalid characters.
 */
export function filterToolNameChars(raw: string): string {
  return raw.replace(/[^a-z0-9_]/g, '');
}
