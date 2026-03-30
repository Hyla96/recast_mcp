import { describe, it, expect } from 'vitest';
import { parseJsonWithLineNumbers } from '../jsonValidator';

describe('parseJsonWithLineNumbers', () => {
  // ── Valid JSON ──────────────────────────────────────────────────────────────

  it('returns ok:true for a valid object', () => {
    const result = parseJsonWithLineNumbers('{"key": "value"}');
    expect(result).toEqual({ ok: true, value: { key: 'value' } });
  });

  it('returns ok:true for a valid array', () => {
    const result = parseJsonWithLineNumbers('[1, 2, 3]');
    expect(result).toEqual({ ok: true, value: [1, 2, 3] });
  });

  it('returns ok:true for null', () => {
    const result = parseJsonWithLineNumbers('null');
    expect(result).toEqual({ ok: true, value: null });
  });

  it('returns ok:true for a boolean', () => {
    expect(parseJsonWithLineNumbers('true')).toEqual({ ok: true, value: true });
    expect(parseJsonWithLineNumbers('false')).toEqual({ ok: true, value: false });
  });

  it('returns ok:true for a number', () => {
    expect(parseJsonWithLineNumbers('42')).toEqual({ ok: true, value: 42 });
    expect(parseJsonWithLineNumbers('3.14')).toEqual({ ok: true, value: 3.14 });
  });

  it('returns ok:true for a string', () => {
    expect(parseJsonWithLineNumbers('"hello"')).toEqual({ ok: true, value: 'hello' });
  });

  it('returns ok:true for a nested structure', () => {
    const input = '{"a": {"b": [1, 2]}, "c": null}';
    const result = parseJsonWithLineNumbers(input);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value).toEqual({ a: { b: [1, 2] }, c: null });
    }
  });

  it('returns ok:true for prettily formatted JSON', () => {
    const input = '{\n  "name": "Alice",\n  "age": 30\n}';
    const result = parseJsonWithLineNumbers(input);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value).toEqual({ name: 'Alice', age: 30 });
    }
  });

  // ── Invalid JSON ────────────────────────────────────────────────────────────

  it('returns ok:false for an empty string', () => {
    const result = parseJsonWithLineNumbers('');
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(typeof result.error).toBe('string');
      expect(result.error.length).toBeGreaterThan(0);
    }
  });

  it('returns ok:false for bare text', () => {
    const result = parseJsonWithLineNumbers('not json at all');
    expect(result.ok).toBe(false);
  });

  it('returns ok:false for trailing comma in object', () => {
    const result = parseJsonWithLineNumbers('{"a": 1,}');
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(typeof result.error).toBe('string');
    }
  });

  it('returns ok:false for unclosed object', () => {
    const result = parseJsonWithLineNumbers('{"key": "value"');
    expect(result.ok).toBe(false);
  });

  it('returns ok:false for unclosed array', () => {
    const result = parseJsonWithLineNumbers('[1, 2, 3');
    expect(result.ok).toBe(false);
  });

  it('returns ok:false for single-quoted strings', () => {
    const result = parseJsonWithLineNumbers("{'key': 'value'}");
    expect(result.ok).toBe(false);
  });

  // ── Line number extraction ──────────────────────────────────────────────────

  it('returns a number or null for line (never undefined)', () => {
    const result = parseJsonWithLineNumbers('{bad}');
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.line === null || typeof result.line === 'number').toBe(true);
    }
  });

  it('computes line 1 for a single-line syntax error', () => {
    // Bare word on line 1 — error should be on line 1 if position is available
    const result = parseJsonWithLineNumbers('{invalid}');
    expect(result.ok).toBe(false);
    if (!result.ok && result.line !== null) {
      expect(result.line).toBe(1);
    }
  });

  it('computes line 2 for an error on the second line', () => {
    // Line 1: '{' — valid
    // Line 2: '  "key": invalid_value' — error here at "invalid_value"
    // Line 3: '}'
    const input = '{\n  "key": invalid_value\n}';
    const result = parseJsonWithLineNumbers(input);
    expect(result.ok).toBe(false);
    if (!result.ok && result.line !== null) {
      // Error is on line 2 (the "invalid_value" token)
      expect(result.line).toBe(2);
    }
  });

  it('computes line 3 for an error on the third line', () => {
    // Line 1: '{'
    // Line 2: '  "a": 1,'
    // Line 3: '  "b": bad_token'
    // Line 4: '}'
    const input = '{\n  "a": 1,\n  "b": bad_token\n}';
    const result = parseJsonWithLineNumbers(input);
    expect(result.ok).toBe(false);
    if (!result.ok && result.line !== null) {
      expect(result.line).toBe(3);
    }
  });

  it('returns a string error message for all failures', () => {
    const cases = [
      '',
      '{',
      '[1,]',
      'undefined',
      "{'x': 1}",
    ] as const;

    for (const input of cases) {
      const result = parseJsonWithLineNumbers(input);
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(typeof result.error).toBe('string');
        expect(result.error.length).toBeGreaterThan(0);
      }
    }
  });
});
