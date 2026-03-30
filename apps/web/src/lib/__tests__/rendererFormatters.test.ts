import { describe, it, expect } from 'vitest';
import { formatFieldName, formatValue, buildJsonPath } from '../rendererFormatters';

// ── formatFieldName ───────────────────────────────────────────────────────────

describe('formatFieldName', () => {
  // ── Basic cases ───────────────────────────────────────────────────────────

  it('title-cases a plain lowercase word', () => {
    expect(formatFieldName('name')).toBe('Name');
  });

  it('returns a single uppercase word with the rest lowercased', () => {
    expect(formatFieldName('NAME')).toBe('Name');
  });

  // ── snake_case ────────────────────────────────────────────────────────────

  it('splits snake_case into title-cased words', () => {
    expect(formatFieldName('first_name')).toBe('First Name');
  });

  it('splits multi-segment snake_case', () => {
    expect(formatFieldName('created_at_utc')).toBe('Created At Utc');
  });

  // ── camelCase / PascalCase ────────────────────────────────────────────────

  it('splits camelCase', () => {
    expect(formatFieldName('firstName')).toBe('First Name');
  });

  it('splits PascalCase', () => {
    expect(formatFieldName('FirstName')).toBe('First Name');
  });

  it('splits already title-cased PascalCase (UserName)', () => {
    expect(formatFieldName('UserName')).toBe('User Name');
  });

  // ── acronym boundaries ────────────────────────────────────────────────────

  it('splits acronym + word boundary (XMLParser → Xml Parser)', () => {
    expect(formatFieldName('XMLParser')).toBe('Xml Parser');
  });

  it('splits acronym at end (parseURL → Parse Url)', () => {
    // parseURL: step2 finds 'e'+'U' boundary → "parse URL"
    // step3 finds 'UR'+'L' — 'R' is uppercase, 'L' is uppercase but 'L' alone can't trigger step1.
    // Actually "parseURL": step1 no match; step2: 'e'+'U' → "parse URL"; split → ["parse","URL"];
    // title-case: ["Parse", "Url"]. Join: "Parse Url".
    expect(formatFieldName('parseURL')).toBe('Parse Url');
  });

  // ── mixed snake + camel ───────────────────────────────────────────────────

  it('splits mixed snake_case + camelCase', () => {
    expect(formatFieldName('user_firstName')).toBe('User First Name');
  });

  // ── hyphen-separated ──────────────────────────────────────────────────────

  it('splits hyphen-separated keys', () => {
    expect(formatFieldName('created-at')).toBe('Created At');
  });

  // ── digits ────────────────────────────────────────────────────────────────

  it('handles digit-uppercase boundary (field1Name → Field1 Name)', () => {
    expect(formatFieldName('field1Name')).toBe('Field1 Name');
  });

  // ── leading underscores ───────────────────────────────────────────────────

  it('strips a single leading underscore', () => {
    expect(formatFieldName('_id')).toBe('Id');
  });

  it('strips multiple leading underscores', () => {
    expect(formatFieldName('__type')).toBe('Type');
  });

  it('returns original key when the entire string is underscores', () => {
    expect(formatFieldName('___')).toBe('___');
  });

  it('strips underscores then splits camelCase remainder (_firstName)', () => {
    expect(formatFieldName('_firstName')).toBe('First Name');
  });
});

// ── formatValue ───────────────────────────────────────────────────────────────

describe('formatValue', () => {
  // ── null / undefined ─────────────────────────────────────────────────────

  it('formats null as an em dash with type null', () => {
    const result = formatValue(null);
    expect(result.type).toBe('null');
    expect(result.display).toBe('—');
    expect(result.raw).toBeNull();
  });

  it('formats undefined as an em dash with type null', () => {
    const result = formatValue(undefined);
    expect(result.type).toBe('null');
    expect(result.display).toBe('—');
    expect(result.raw).toBeNull();
  });

  // ── boolean ──────────────────────────────────────────────────────────────

  it('formats true as "Yes"', () => {
    const result = formatValue(true);
    expect(result.type).toBe('boolean');
    expect(result.display).toBe('Yes');
    expect(result.raw).toBe(true);
  });

  it('formats false as "No"', () => {
    const result = formatValue(false);
    expect(result.type).toBe('boolean');
    expect(result.display).toBe('No');
    expect(result.raw).toBe(false);
  });

  // ── number — plain ────────────────────────────────────────────────────────

  it('formats a plain integer as a number', () => {
    const result = formatValue(1234);
    expect(result.type).toBe('number');
    expect(result.raw).toBe(1234);
    // The display should contain the digits in some locale-formatted form.
    expect(result.display).toMatch(/1[,\s.]?2[,\s.]?3[,\s.]?4/);
  });

  it('formats zero', () => {
    const result = formatValue(0);
    expect(result.type).toBe('number');
    expect(result.raw).toBe(0);
    expect(result.display.length).toBeGreaterThan(0);
  });

  it('formats a negative number', () => {
    const result = formatValue(-42.5);
    expect(result.type).toBe('number');
    expect(result.display).toContain('42');
  });

  it('formats without a field key as a plain number (no currency/percent symbol)', () => {
    const result = formatValue(99.9);
    expect(result.type).toBe('number');
    expect(result.display).not.toContain('%');
  });

  it('does not apply currency format for an unrelated key', () => {
    const result = formatValue(1234, 'name');
    expect(result.type).toBe('number');
    expect(result.display).not.toContain('$');
    expect(result.display).not.toContain('€');
  });

  // ── number — currency heuristic ───────────────────────────────────────────

  it('applies currency format for "price" field key', () => {
    const result = formatValue(9.99, 'price');
    expect(result.type).toBe('number');
    expect(result.raw).toBe(9.99);
    // Must contain the numeric value.
    expect(result.display).toMatch(/9[.,]99/);
  });

  it('applies currency format for "total" field key', () => {
    const result = formatValue(1500, 'total');
    expect(result.type).toBe('number');
    expect(result.raw).toBe(1500);
  });

  it('applies currency format for "amount" field key', () => {
    const result = formatValue(250.0, 'amount');
    expect(result.type).toBe('number');
  });

  it('applies currency format for "cost" field key', () => {
    const result = formatValue(75.5, 'cost');
    expect(result.type).toBe('number');
  });

  it('applies currency format for "balance" field key', () => {
    const result = formatValue(10000, 'balance');
    expect(result.type).toBe('number');
  });

  // ── number — percent heuristic ────────────────────────────────────────────

  it('applies percent format for "percent" field key with fraction (0–1 range)', () => {
    const result = formatValue(0.42, 'percent');
    expect(result.type).toBe('number');
    expect(result.raw).toBe(0.42);
    expect(result.display).toContain('%');
  });

  it('applies percent format for "rate" field key with whole-number percentage', () => {
    const result = formatValue(42, 'rate');
    expect(result.type).toBe('number');
    expect(result.raw).toBe(42);
    expect(result.display).toContain('%');
  });

  it('applies percent format for "ratio" field key', () => {
    const result = formatValue(0.75, 'ratio');
    expect(result.type).toBe('number');
    expect(result.display).toContain('%');
  });

  it('applies percent format for "pct" field key', () => {
    const result = formatValue(15, 'pct');
    expect(result.type).toBe('number');
    expect(result.display).toContain('%');
  });

  // ── string — plain ────────────────────────────────────────────────────────

  it('returns a plain string as-is', () => {
    const result = formatValue('hello world');
    expect(result.type).toBe('string');
    expect(result.display).toBe('hello world');
    expect(result.raw).toBe('hello world');
  });

  it('returns an empty string as-is', () => {
    const result = formatValue('');
    expect(result.type).toBe('string');
    expect(result.display).toBe('');
  });

  it('returns a numeric-looking string as a string', () => {
    const result = formatValue('12345');
    expect(result.type).toBe('string');
    expect(result.display).toBe('12345');
  });

  it('returns a non-date string with a date-like prefix as a string', () => {
    const result = formatValue('hello-2024-01-15');
    expect(result.type).toBe('string');
    expect(result.display).toBe('hello-2024-01-15');
  });

  // ── string — ISO date ─────────────────────────────────────────────────────

  it('formats an ISO date-only string as type date', () => {
    const result = formatValue('2024-01-15');
    expect(result.type).toBe('date');
    expect(result.raw).toBe('2024-01-15');
    // Display must not be the raw ISO string.
    expect(result.display).not.toBe('2024-01-15');
    expect(result.display.length).toBeGreaterThan(0);
  });

  it('formats an ISO datetime string (UTC) as type date', () => {
    const result = formatValue('2024-06-01T10:30:00Z');
    expect(result.type).toBe('date');
    expect(result.raw).toBe('2024-06-01T10:30:00Z');
    expect(result.display).not.toBe('2024-06-01T10:30:00Z');
  });

  it('formats an ISO datetime string with timezone offset as type date', () => {
    const result = formatValue('2024-06-01T12:00:00+05:30');
    expect(result.type).toBe('date');
    expect(result.display.length).toBeGreaterThan(0);
  });

  it('formats an ISO datetime with seconds and milliseconds as type date', () => {
    const result = formatValue('2024-03-15T09:45:00.123Z');
    expect(result.type).toBe('date');
  });

  it('returns an invalid ISO-formatted date as a plain string', () => {
    // Regex matches, but Date('2024-99-99') is an invalid date.
    const result = formatValue('2024-99-99');
    expect(result.type).toBe('string');
    expect(result.display).toBe('2024-99-99');
  });

  // ── fallback ──────────────────────────────────────────────────────────────

  it('converts a BigInt to a string display (fallback)', () => {
    const result = formatValue(BigInt(42));
    expect(result.type).toBe('string');
    expect(result.display).toBe('42');
  });
});

// ── buildJsonPath ─────────────────────────────────────────────────────────────

describe('buildJsonPath', () => {
  // ── Root-level keys ───────────────────────────────────────────────────────

  it('builds a root-level string key with dot notation', () => {
    expect(buildJsonPath('name', '$')).toBe('$.name');
  });

  it('builds a root-level numeric index', () => {
    expect(buildJsonPath(0, '$')).toBe('$[0]');
  });

  it('uses $ as the base when parentPath is empty', () => {
    expect(buildJsonPath('name', '')).toBe('$.name');
  });

  // ── Nested keys ───────────────────────────────────────────────────────────

  it('appends a string key with dot notation to a nested path', () => {
    expect(buildJsonPath('city', '$.address')).toBe('$.address.city');
  });

  it('appends an array index to a nested path', () => {
    expect(buildJsonPath(0, '$.items')).toBe('$.items[0]');
  });

  it('appends a string key after an array index', () => {
    expect(buildJsonPath('name', '$.items[0]')).toBe('$.items[0].name');
  });

  it('supports non-zero array indices', () => {
    expect(buildJsonPath(3, '$.results')).toBe('$.results[3]');
  });

  // ── Deep nesting ──────────────────────────────────────────────────────────

  it('chains multiple buildJsonPath calls correctly', () => {
    const level1 = buildJsonPath('items', '$');    // $.items
    const level2 = buildJsonPath(2, level1);       // $.items[2]
    const level3 = buildJsonPath('title', level2); // $.items[2].title
    expect(level3).toBe('$.items[2].title');
  });

  // ── Bracket notation for special keys ─────────────────────────────────────

  it('uses bracket notation for keys with spaces', () => {
    expect(buildJsonPath('first name', '$')).toBe("$['first name']");
  });

  it('uses bracket notation for keys with hyphens', () => {
    expect(buildJsonPath('created-at', '$')).toBe("$['created-at']");
  });

  it('uses bracket notation for keys starting with a digit', () => {
    expect(buildJsonPath('1key', '$')).toBe("$['1key']");
  });

  it('uses bracket notation for keys with dots', () => {
    expect(buildJsonPath('user.id', '$')).toBe("$['user.id']");
  });

  // ── Valid identifier keys ─────────────────────────────────────────────────

  it('uses dot notation for underscore-prefixed identifier', () => {
    expect(buildJsonPath('_id', '$')).toBe('$._id');
  });

  it('uses dot notation for $ prefixed identifier', () => {
    expect(buildJsonPath('$ref', '$')).toBe('$.$ref');
  });

  it('uses dot notation for alphanumeric identifier', () => {
    expect(buildJsonPath('user_id', '$.data')).toBe('$.data.user_id');
  });
});
