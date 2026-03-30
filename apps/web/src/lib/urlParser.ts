/**
 * URL parser for the builder URL step.
 *
 * Parses a REST API URL into path params (from `{name}` placeholders) and
 * query params (from the query string). No network requests; purely
 * synchronous.
 */

export type ParamType = 'string' | 'number' | 'boolean';

export interface ParsedPathParam {
  name: string;
  type: ParamType;
  example: string;
}

export interface ParsedQueryParam {
  key: string;
  type: ParamType;
  rawValue: string;
}

export type UrlErrorCode =
  | 'EMPTY'
  | 'INVALID_URL'
  | 'NO_PROTOCOL'
  | 'RELATIVE_URL'
  | 'UNSUPPORTED_PROTOCOL';

export interface ParsedUrl {
  pathParams: ParsedPathParam[];
  queryParams: ParsedQueryParam[];
  isValid: boolean;
  error?: UrlErrorCode;
  /** Normalised URL with the query string stripped (params are in queryParams). */
  baseUrl: string;
}

const PATH_PARAM_RE = /\{([a-zA-Z_][a-zA-Z0-9_-]*)\}/g;

/**
 * Infer the most-specific ParamType from a raw string value.
 * - Recognisable booleans → 'boolean'
 * - Pure integers or decimals → 'number'
 * - Anything else → 'string'
 */
function inferType(value: string): ParamType {
  const lower = value.toLowerCase();
  if (lower === 'true' || lower === 'false') return 'boolean';
  if (value !== '' && !Number.isNaN(Number(value))) return 'number';
  return 'string';
}

/**
 * Parse a REST API URL into path params, query params, and validity state.
 *
 * Error codes:
 *   EMPTY             — url is blank / whitespace-only
 *   NO_PROTOCOL       — url looks like a hostname but has no scheme
 *   RELATIVE_URL      — url starts with `/` (path-only)
 *   UNSUPPORTED_PROTOCOL — scheme is not http or https
 *   INVALID_URL       — anything else that the URL constructor rejects
 */
export function parseRestUrl(url: string): ParsedUrl {
  const trimmed = url.trim();

  if (trimmed === '') {
    return { pathParams: [], queryParams: [], isValid: false, error: 'EMPTY', baseUrl: '' };
  }

  // Relative paths (start with '/') are not absolute REST API URLs.
  if (trimmed.startsWith('/')) {
    return {
      pathParams: [],
      queryParams: [],
      isValid: false,
      error: 'RELATIVE_URL',
      baseUrl: '',
    };
  }

  // Likely hostname without a protocol (e.g. "api.example.com/…").
  if (!trimmed.includes('://')) {
    return {
      pathParams: [],
      queryParams: [],
      isValid: false,
      error: 'NO_PROTOCOL',
      baseUrl: '',
    };
  }

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return { pathParams: [], queryParams: [], isValid: false, error: 'INVALID_URL', baseUrl: '' };
  }

  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    return {
      pathParams: [],
      queryParams: [],
      isValid: false,
      error: 'UNSUPPORTED_PROTOCOL',
      baseUrl: trimmed,
    };
  }

  // --- Path params ---
  // The URL constructor percent-encodes `{` → `%7B` and `}` → `%7D` in the
  // pathname, so we must decode before running the regex.
  const decodedPathname = decodeURIComponent(parsed.pathname);
  const pathParams: ParsedPathParam[] = [];
  const seen = new Set<string>();
  let match: RegExpExecArray | null;
  // Reset lastIndex before exec loop.
  PATH_PARAM_RE.lastIndex = 0;
  while ((match = PATH_PARAM_RE.exec(decodedPathname)) !== null) {
    const name = match[1];
    if (name !== undefined && !seen.has(name)) {
      seen.add(name);
      pathParams.push({ name, type: 'string', example: '' });
    }
  }

  // --- Query params ---
  const queryParams: ParsedQueryParam[] = [];
  parsed.searchParams.forEach((rawValue, key) => {
    queryParams.push({ key, type: inferType(rawValue), rawValue });
  });

  // Base URL without query string or fragment.
  const baseUrl = `${parsed.origin}${parsed.pathname}`;

  return { pathParams, queryParams, isValid: true, baseUrl };
}
