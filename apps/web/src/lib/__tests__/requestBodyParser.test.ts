import { describe, it, expect } from 'vitest';
import { extractTemplateVars } from '@/lib/requestBodyParser';

describe('extractTemplateVars', () => {
  it('returns [] for an empty string', () => {
    expect(extractTemplateVars('')).toEqual([]);
  });

  it('returns [] for body with no template vars', () => {
    expect(extractTemplateVars('{"name": "Alice", "age": 30}')).toEqual([]);
  });

  it('detects a quoted string variable', () => {
    expect(extractTemplateVars('{"name": "{{name}}"}')).toEqual([
      { name: 'name', type: 'string' },
    ]);
  });

  it('detects an unquoted number variable', () => {
    expect(extractTemplateVars('{"count": {{count}}}')).toEqual([
      { name: 'count', type: 'number' },
    ]);
  });

  it('detects multiple vars of mixed types', () => {
    expect(
      extractTemplateVars('{"label": "{{label}}", "qty": {{qty}}}')
    ).toEqual([
      { name: 'label', type: 'string' },
      { name: 'qty', type: 'number' },
    ]);
  });

  it('deduplicates by name — first occurrence wins for type', () => {
    // First occurrence is quoted (string); second is unquoted (would be number).
    const result = extractTemplateVars('{"a": "{{foo}}", "b": {{foo}}}');
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ name: 'foo', type: 'string' });
  });

  it('preserves first-occurrence order', () => {
    const result = extractTemplateVars('{"z": "{{z}}", "a": {{a}}}');
    expect(result.map((v) => v.name)).toEqual(['z', 'a']);
  });

  it('handles names with underscores and trailing digits', () => {
    expect(extractTemplateVars('{"v": "{{user_id_123}}"}')).toEqual([
      { name: 'user_id_123', type: 'string' },
    ]);
  });

  it('handles names starting with an underscore', () => {
    expect(extractTemplateVars('{"v": "{{_private}}"}')).toEqual([
      { name: '_private', type: 'string' },
    ]);
  });

  it('does not match placeholders starting with a digit', () => {
    // {{1bad}} does not satisfy [a-zA-Z_][a-zA-Z0-9_]*
    expect(extractTemplateVars('{"v": {{1bad}}}')).toEqual([]);
  });

  it('detects vars inside nested objects', () => {
    const body = '{"user": {"id": "{{user_id}}", "age": {{age}}}}';
    expect(extractTemplateVars(body)).toEqual([
      { name: 'user_id', type: 'string' },
      { name: 'age', type: 'number' },
    ]);
  });

  it('treats placeholder with whitespace between outer quotes as string', () => {
    // "  {{x}}  " — whitespace between the enclosing quotes and the braces
    const result = extractTemplateVars('{"v": "  {{x}}  "}');
    expect(result).toEqual([{ name: 'x', type: 'string' }]);
  });

  it('counts each unique name only once even with many occurrences', () => {
    const body = '{"a": "{{name}}", "b": "{{name}}", "c": {{name}}}';
    const result = extractTemplateVars(body);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ name: 'name', type: 'string' });
  });

  it('returns [] for a non-JSON string with no placeholders', () => {
    expect(extractTemplateVars('not json at all')).toEqual([]);
  });

  it('detects a bare-value placeholder at root level', () => {
    // A placeholder that is the entire JSON value (unusual but valid template)
    const result = extractTemplateVars('{{root_value}}');
    expect(result).toEqual([{ name: 'root_value', type: 'number' }]);
  });
});
