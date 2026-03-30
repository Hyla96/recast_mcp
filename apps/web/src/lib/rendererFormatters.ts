/**
 * Pure formatting helpers for the DocumentRenderer.
 *
 * All functions are side-effect-free and take only plain values,
 * making them trivially unit-testable.
 */

// ── Types ─────────────────────────────────────────────────────────────────────

export type ScalarType = 'string' | 'number' | 'boolean' | 'null' | 'date';

export interface FormattedValue {
  type: ScalarType;
  /** Human-readable display string. */
  display: string;
  /** Original raw value (for onFieldSelect callbacks). */
  raw: unknown;
}

// ── Heuristic patterns ────────────────────────────────────────────────────────

/** Field keys whose values are likely monetary amounts. */
const CURRENCY_KEY_RE = /price|amount|cost|fee|payment|salary|revenue|total|balance|budget/i;

/** Field keys whose values are likely ratios or percentages. */
const PERCENT_KEY_RE = /percent|pct|rate|ratio|share|fraction/i;

/**
 * ISO 8601 date-only (YYYY-MM-DD) or full datetime
 * (YYYY-MM-DDTHH:MM[:SS[.fff]][Z|±HH:MM]).
 * Does NOT validate calendar correctness (e.g. 2024-99-99 passes the regex
 * but produces an invalid Date, which is handled downstream).
 */
const ISO_DATE_RE =
  /^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$/;

// ── formatFieldName ───────────────────────────────────────────────────────────

/**
 * Converts an object key to a human-friendly label.
 *
 * Algorithm (applied in order):
 * 1. Strip leading underscores.
 * 2. Split acronym runs (e.g. "XMLParser" → "XML Parser").
 * 3. Split camelCase / PascalCase boundaries.
 * 4. Split on remaining underscores, hyphens, and spaces.
 * 5. Title-case each word and join with a single space.
 *
 * Edge case: if the key is entirely underscores, return it unchanged.
 */
export function formatFieldName(key: string): string {
  const stripped = key.replace(/^_+/, '');
  if (!stripped) return key; // all underscores — no useful label

  const words = stripped
    // "XMLParser" → "XML Parser" (uppercase run + uppercase+lowercase)
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
    // "camelCase" → "camel Case"
    .replace(/([a-z\d])([A-Z])/g, '$1 $2')
    // split on underscore, hyphen, or whitespace
    .split(/[_\s-]+/)
    .filter(Boolean);

  return words
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase())
    .join(' ');
}

// ── formatValue ───────────────────────────────────────────────────────────────

/**
 * Formats a scalar value for human display.
 *
 * - `null` / `undefined` → '—' (em dash), type 'null'
 * - `boolean`            → 'Yes' / 'No', type 'boolean'
 * - `number`             → locale-formatted via Intl.NumberFormat; currency/
 *                          percent style when `fieldKey` matches heuristics
 * - `string`             → ISO 8601 date/datetime strings formatted via
 *                          Intl.DateTimeFormat, type 'date'; everything else
 *                          returned as-is, type 'string'
 * - any other type       → String(value), type 'string'
 */
export function formatValue(value: unknown, fieldKey?: string | undefined): FormattedValue {
  // ── null / undefined ────────────────────────────────────────────────────
  if (value === null || value === undefined) {
    return { type: 'null', display: '—', raw: null };
  }

  // ── boolean ─────────────────────────────────────────────────────────────
  if (typeof value === 'boolean') {
    return { type: 'boolean', display: value ? 'Yes' : 'No', raw: value };
  }

  // ── number ──────────────────────────────────────────────────────────────
  if (typeof value === 'number') {
    if (fieldKey !== undefined && CURRENCY_KEY_RE.test(fieldKey)) {
      try {
        return {
          type: 'number',
          display: new Intl.NumberFormat(undefined, {
            style: 'currency',
            currency: 'USD',
            minimumFractionDigits: 2,
          }).format(value),
          raw: value,
        };
      } catch {
        // Intl.NumberFormat with 'currency' style unsupported — fall through.
      }
    }

    if (fieldKey !== undefined && PERCENT_KEY_RE.test(fieldKey)) {
      // Values > 1 are assumed to be already expressed as percentages (e.g. 42)
      // and are normalised to a fraction (0.42) for Intl.NumberFormat.
      const normalised = value > 1 ? value / 100 : value;
      return {
        type: 'number',
        display: new Intl.NumberFormat(undefined, {
          style: 'percent',
          minimumFractionDigits: 0,
          maximumFractionDigits: 2,
        }).format(normalised),
        raw: value,
      };
    }

    return {
      type: 'number',
      display: new Intl.NumberFormat().format(value),
      raw: value,
    };
  }

  // ── string ──────────────────────────────────────────────────────────────
  if (typeof value === 'string') {
    if (ISO_DATE_RE.test(value)) {
      try {
        const date = new Date(value);
        if (!isNaN(date.getTime())) {
          const hasTime = value.includes('T');
          const opts: Intl.DateTimeFormatOptions = {
            year: 'numeric',
            month: 'long',
            day: 'numeric',
          };
          if (hasTime) {
            opts.hour = 'numeric';
            opts.minute = 'numeric';
          }
          return {
            type: 'date',
            display: new Intl.DateTimeFormat(undefined, opts).format(date),
            raw: value,
          };
        }
      } catch {
        // Invalid date — fall through to plain string.
      }
    }
    return { type: 'string', display: value, raw: value };
  }

  // ── fallback ─────────────────────────────────────────────────────────────
  return { type: 'string', display: String(value), raw: value };
}

// ── buildJsonPath ─────────────────────────────────────────────────────────────

/**
 * Builds a JSONPath expression for a key within a parent path.
 *
 * - Numeric keys always use bracket notation: `$.items[0]`
 * - String keys that are valid JS identifiers use dot notation: `$.name`
 * - All other string keys use quoted bracket notation: `$['first name']`
 *
 * When `parentPath` is empty it defaults to `$` (JSONPath root).
 */
export function buildJsonPath(key: string | number, parentPath: string): string {
  const base = parentPath || '$';

  if (typeof key === 'number') {
    return `${base}[${key}]`;
  }

  const isIdentifier = /^[a-zA-Z_$][a-zA-Z0-9_$]*$/.test(key);
  return isIdentifier ? `${base}.${key}` : `${base}['${key}']`;
}
