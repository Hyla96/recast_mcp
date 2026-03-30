/**
 * JSON parsing with line-number extraction on failure.
 *
 * Used by:
 *  - Sample JSON escape hatch (TASK-008)
 *  - Request body builder (TASK-009)
 */

export type JsonParseResult =
  | { ok: true; value: unknown }
  | { ok: false; error: string; line: number | null };

/**
 * Attempts to parse `text` as JSON.
 *
 * On success: returns `{ ok: true, value }`.
 * On failure: returns `{ ok: false, error, line }` where `line` is the 1-based
 * line number extracted from the SyntaxError message, or `null` if it could
 * not be determined.
 *
 * Line number extraction is attempted in two strategies (tried in order):
 *  1. "at position N" — Node.js v20+ / modern Chrome. Converts character
 *     offset to a 1-based line number by counting newlines before it.
 *  2. "at line N" — Firefox error message format. Parsed directly.
 */
export function parseJsonWithLineNumbers(text: string): JsonParseResult {
  try {
    const value: unknown = JSON.parse(text);
    return { ok: true, value };
  } catch (e) {
    if (!(e instanceof SyntaxError)) {
      return { ok: false, error: String(e), line: null };
    }

    const msg = e.message;
    let line: number | null = null;

    // Strategy 1: character position → line number (Node.js v20+ / Chrome modern)
    const posMatch = /\bat position (\d+)/i.exec(msg);
    if (posMatch?.[1] !== undefined) {
      const pos = parseInt(posMatch[1], 10);
      const before = text.slice(0, pos);
      line = (before.match(/\n/g) ?? []).length + 1;
    }

    // Strategy 2: explicit line number in message (Firefox)
    if (line === null) {
      const lineMatch = /\bat line (\d+)/i.exec(msg);
      if (lineMatch?.[1] !== undefined) {
        line = parseInt(lineMatch[1], 10);
      }
    }

    return { ok: false, error: msg, line };
  }
}

/**
 * Schedules a function to run during browser idle time (or as a
 * `setTimeout(fn, 0)` fallback in environments that do not support
 * `requestIdleCallback`). Returns a cancel function.
 */
export function scheduleIdleValidation(fn: () => void): () => void {
  if (typeof requestIdleCallback !== 'undefined') {
    const id = requestIdleCallback(() => fn());
    return () => cancelIdleCallback(id);
  }
  const id = setTimeout(fn, 0);
  return () => clearTimeout(id);
}
