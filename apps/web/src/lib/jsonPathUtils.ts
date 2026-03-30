/**
 * JSONPath utility functions.
 *
 * Provides path normalization and navigation helpers used by the
 * array path normalization feature (TASK-011).
 *
 * All exported functions are pure and side-effect-free.
 */

// ── Types ─────────────────────────────────────────────────────────────────────

export interface ArrayContext {
  /** Human-readable name of the array field (e.g. "items", "orders"). */
  arrayName: string;
  /** Total number of items in the array. */
  count: number;
  /**
   * Up to 5 sample values extracted from array items at the subpath after
   * the index. Populated only for the innermost (last) array context.
   */
  previewValues: unknown[];
  /**
   * True when previewValues contains more than one primitive type,
   * indicating a heterogeneous (mixed-type) array.
   */
  hasMixedTypes: boolean;
}

// ── Internal navigation helpers ───────────────────────────────────────────────

/**
 * Parses a JSONPath string into a list of key/index segments.
 *
 * Handles:
 * - Dot notation:              $.name        -> ["name"]
 * - Numeric bracket notation:  $.items[0]    -> ["items", 0]
 * - Quoted bracket notation:   $['foo bar']  -> ["foo bar"]
 *
 * Leading "$" is stripped before parsing. Relative paths (no "$") work too.
 */
function parsePathSegments(path: string): (string | number)[] {
  const segments: (string | number)[] = [];
  let p = path.startsWith('$') ? path.slice(1) : path;

  while (p.length > 0) {
    if (p.startsWith('.')) {
      p = p.slice(1);
      const m = /^([a-zA-Z_$][a-zA-Z0-9_$]*)/.exec(p);
      if (m !== null && m[1] !== undefined) {
        segments.push(m[1]);
        p = p.slice(m[1].length);
      } else {
        break;
      }
    } else if (p.startsWith('[')) {
      p = p.slice(1);
      if (p.startsWith("'")) {
        // Quoted bracket notation: ['foo bar']
        const end = p.indexOf("']");
        if (end !== -1) {
          segments.push(p.slice(1, end));
          p = p.slice(end + 2);
        } else {
          break;
        }
      } else {
        // Numeric index: [0], [123]
        const m = /^(\d+)\]/.exec(p);
        if (m !== null && m[1] !== undefined) {
          segments.push(parseInt(m[1], 10));
          p = p.slice(m[1].length + 1);
        } else {
          break;
        }
      }
    } else {
      break;
    }
  }

  return segments;
}

/**
 * Navigates a data structure by a list of key/index segments.
 * Returns undefined if any step is missing or the type is incompatible.
 */
function navigateSegments(data: unknown, segments: (string | number)[]): unknown {
  let current: unknown = data;
  for (const seg of segments) {
    if (current === null || current === undefined) return undefined;
    if (typeof seg === 'number') {
      if (Array.isArray(current)) {
        current = current[seg];
      } else {
        return undefined;
      }
    } else {
      if (typeof current === 'object' && !Array.isArray(current)) {
        current = (current as Record<string, unknown>)[seg];
      } else {
        return undefined;
      }
    }
  }
  return current;
}

/** Navigate from root using a full or relative JSONPath string. */
function navigateByPath(data: unknown, jsonPath: string): unknown {
  return navigateSegments(data, parsePathSegments(jsonPath));
}

/**
 * Navigate an array item using the subpath that follows an array index.
 * An empty relativePath returns the item itself.
 */
function navigateRelativePath(item: unknown, relativePath: string): unknown {
  if (!relativePath) return item;
  return navigateSegments(item, parsePathSegments(relativePath));
}

/**
 * Extracts the last human-readable key name from a JSONPath prefix.
 * Used to derive the array field name shown in the dialog.
 *
 * Examples:
 *   $.items              -> "items"
 *   $.orders[0].items    -> "items"
 *   $['foo bar']         -> "foo bar"
 */
function extractLastKey(prefixPath: string): string {
  // Dot-notation identifier at end: $.items or $.orders[0].items
  const dotMatch = /\.([a-zA-Z_$][a-zA-Z0-9_$]*)$/.exec(prefixPath);
  if (dotMatch !== null && dotMatch[1] !== undefined) return dotMatch[1];
  // Quoted bracket notation at end: $['foo bar']
  const bracketMatch = /\['([^']+)'\]$/.exec(prefixPath);
  if (bracketMatch !== null && bracketMatch[1] !== undefined) return bracketMatch[1];
  return 'items';
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Normalizes a JSONPath by replacing all numeric array indices with [*].
 *
 * Only /\[(\d+)\]/g is replaced — [*] and ['key'] are left unchanged.
 *
 * Examples:
 *   $.items[0].price         -> $.items[*].price
 *   $.orders[0].items[1].sku -> $.orders[*].items[*].sku
 *   $.name                   -> $.name  (unchanged)
 *   $.items[2]               -> $.items[*]
 */
export function normalizeArrayPath(jsonPath: string): string {
  return jsonPath.replace(/\[(\d+)\]/g, '[*]');
}

/**
 * Returns true when the JSONPath contains at least one numeric array index.
 *
 * Detection regex: /\[(\d+)\]/ — matches [0], [1], [123].
 * Does NOT match [*] or ['key'].
 */
export function hasArrayIndex(jsonPath: string): boolean {
  return /\[(\d+)\]/.test(jsonPath);
}

/**
 * Extracts array context information from a JSONPath and the response data.
 *
 * Returns one ArrayContext per numeric array index found in the path.
 * They are ordered from outermost to innermost array.
 *
 * Preview values and mixed-type detection are computed only for the
 * innermost (last) context, since that is the array the user directly
 * clicked into. Outer contexts receive empty previewValues arrays.
 *
 * Used by ArrayNormalizationDialog to render the confirmation UI.
 */
export function getArrayContexts(jsonPath: string, data: unknown): ArrayContext[] {
  const contexts: ArrayContext[] = [];
  const re = /\[(\d+)\]/g;
  let match: RegExpExecArray | null;
  let lastEntry: { suffixPath: string; arrayData: unknown[] } | null = null;

  while ((match = re.exec(jsonPath)) !== null) {
    const indexStr = match[1];
    if (indexStr === undefined) continue;

    const matchStart = match.index;
    const prefixPath = jsonPath.slice(0, matchStart);
    const suffixPath = jsonPath.slice(matchStart + match[0].length);

    const maybeArray = navigateByPath(data, prefixPath);
    if (!Array.isArray(maybeArray)) continue;

    contexts.push({
      arrayName: extractLastKey(prefixPath),
      count: maybeArray.length,
      previewValues: [],
      hasMixedTypes: false,
    });

    lastEntry = { suffixPath, arrayData: maybeArray };
  }

  // Populate preview values for the innermost (last) array context only.
  if (lastEntry !== null && contexts.length > 0) {
    const { suffixPath, arrayData } = lastEntry;
    const previewValues: unknown[] = [];

    for (let i = 0; i < Math.min(5, arrayData.length); i++) {
      const item = arrayData[i];
      const val = suffixPath ? navigateRelativePath(item, suffixPath) : item;
      previewValues.push(val);
    }

    // Detect mixed primitive types (ignore null/undefined).
    const types = new Set(
      previewValues.filter((v) => v !== null && v !== undefined).map((v) => typeof v)
    );

    const lastCtx = contexts[contexts.length - 1];
    if (lastCtx !== undefined) {
      lastCtx.previewValues = previewValues;
      lastCtx.hasMixedTypes = types.size > 1;
    }
  }

  return contexts;
}
