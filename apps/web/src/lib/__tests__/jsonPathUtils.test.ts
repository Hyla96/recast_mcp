import { describe, test, expect } from 'vitest';
import { normalizeArrayPath, hasArrayIndex, getArrayContexts } from '../jsonPathUtils';

// ── normalizeArrayPath ────────────────────────────────────────────────────────

describe('normalizeArrayPath', () => {
  test('no array - root path unchanged', () => {
    expect(normalizeArrayPath('$')).toBe('$');
  });

  test('no array - simple dot path unchanged', () => {
    expect(normalizeArrayPath('$.name')).toBe('$.name');
  });

  test('no array - nested dot path unchanged', () => {
    expect(normalizeArrayPath('$.user.firstName')).toBe('$.user.firstName');
  });

  test('no array - already normalized path unchanged', () => {
    expect(normalizeArrayPath('$.items[*].price')).toBe('$.items[*].price');
  });

  test('single array - index 0', () => {
    expect(normalizeArrayPath('$.items[0].price')).toBe('$.items[*].price');
  });

  test('single array - index 5', () => {
    expect(normalizeArrayPath('$.items[5].name')).toBe('$.items[*].name');
  });

  test('single array - large index', () => {
    expect(normalizeArrayPath('$.items[123].sku')).toBe('$.items[*].sku');
  });

  test('single array - path ending at array element', () => {
    expect(normalizeArrayPath('$.items[2]')).toBe('$.items[*]');
  });

  test('nested arrays - two levels', () => {
    expect(normalizeArrayPath('$.orders[0].items[1].sku')).toBe('$.orders[*].items[*].sku');
  });

  test('nested arrays - both at zero', () => {
    expect(normalizeArrayPath('$.a[0].b[0].c')).toBe('$.a[*].b[*].c');
  });

  test('nested arrays - path ending at inner element', () => {
    expect(normalizeArrayPath('$.orders[3].items[7]')).toBe('$.orders[*].items[*]');
  });
});

// ── hasArrayIndex ─────────────────────────────────────────────────────────────

describe('hasArrayIndex', () => {
  test('true for path with [0]', () => {
    expect(hasArrayIndex('$.items[0].price')).toBe(true);
  });

  test('true for path ending at index', () => {
    expect(hasArrayIndex('$.items[2]')).toBe(true);
  });

  test('true for large index', () => {
    expect(hasArrayIndex('$.items[99]')).toBe(true);
  });

  test('true for nested indices', () => {
    expect(hasArrayIndex('$.a[0].b[1]')).toBe(true);
  });

  test('false for plain dot path', () => {
    expect(hasArrayIndex('$.items.price')).toBe(false);
  });

  test('false for normalized path with [*]', () => {
    expect(hasArrayIndex('$.items[*].price')).toBe(false);
  });

  test('false for quoted bracket notation', () => {
    expect(hasArrayIndex("$['foo bar'].name")).toBe(false);
  });

  test('false for root only', () => {
    expect(hasArrayIndex('$')).toBe(false);
  });
});

// ── getArrayContexts ──────────────────────────────────────────────────────────

describe('getArrayContexts', () => {
  const sampleData = {
    items: [
      { price: 10.99, name: 'Widget A', active: true },
      { price: 5.5, name: 'Widget B', active: false },
      { price: 2.0, name: 'Widget C', active: true },
    ],
  };

  test('returns empty array when no index in path', () => {
    const result = getArrayContexts('$.name', sampleData);
    expect(result).toHaveLength(0);
  });

  test('returns empty array when navigation fails', () => {
    const result = getArrayContexts('$.nonexistent[0].price', sampleData);
    expect(result).toHaveLength(0);
  });

  test('single array - count', () => {
    const result = getArrayContexts('$.items[0].price', sampleData);
    expect(result).toHaveLength(1);
    expect(result[0]?.count).toBe(3);
  });

  test('single array - arrayName', () => {
    const result = getArrayContexts('$.items[0].price', sampleData);
    expect(result[0]?.arrayName).toBe('items');
  });

  test('single array - preview values', () => {
    const result = getArrayContexts('$.items[0].price', sampleData);
    expect(result[0]?.previewValues).toEqual([10.99, 5.5, 2.0]);
  });

  test('single array - no mixed types for homogeneous numbers', () => {
    const result = getArrayContexts('$.items[0].price', sampleData);
    expect(result[0]?.hasMixedTypes).toBe(false);
  });

  test('single array - limits preview to 5 items', () => {
    const bigData = {
      list: Array.from({ length: 10 }, (_, i) => ({ val: i })),
    };
    const result = getArrayContexts('$.list[0].val', bigData);
    expect(result[0]?.previewValues).toHaveLength(5);
  });

  test('single array - path ending at array element returns the item', () => {
    const result = getArrayContexts('$.items[0]', sampleData);
    expect(result).toHaveLength(1);
    expect(result[0]?.arrayName).toBe('items');
    expect(result[0]?.previewValues[0]).toEqual({ price: 10.99, name: 'Widget A', active: true });
  });

  test('detects mixed types when preview has numbers and strings', () => {
    const mixedData = {
      vals: [{ x: 1 }, { x: 'two' }, { x: 3 }],
    };
    const result = getArrayContexts('$.vals[0].x', mixedData);
    expect(result[0]?.hasMixedTypes).toBe(true);
  });

  test('detects mixed types across booleans and numbers', () => {
    const mixedData = {
      vals: [{ x: true }, { x: 1 }],
    };
    const result = getArrayContexts('$.vals[0].x', mixedData);
    expect(result[0]?.hasMixedTypes).toBe(true);
  });

  test('no mixed types for boolean-only array', () => {
    const result = getArrayContexts('$.items[0].active', sampleData);
    expect(result[0]?.hasMixedTypes).toBe(false);
  });

  test('ignores null/undefined when checking mixed types', () => {
    const sparseData = {
      vals: [{ x: 1 }, { x: null }, { x: 2 }],
    };
    const result = getArrayContexts('$.vals[0].x', sparseData);
    // Only non-null values: 1, 2 — both numbers, not mixed
    expect(result[0]?.hasMixedTypes).toBe(false);
  });

  test('nested arrays - returns two contexts', () => {
    const nestedData = {
      orders: [
        { items: [{ sku: 'A1' }, { sku: 'B1' }] },
        { items: [{ sku: 'A2' }, { sku: 'B2' }] },
      ],
    };
    const result = getArrayContexts('$.orders[0].items[1].sku', nestedData);
    expect(result).toHaveLength(2);
  });

  test('nested arrays - outer context arrayName and count', () => {
    const nestedData = {
      orders: [
        { items: [{ sku: 'A1' }, { sku: 'B1' }] },
        { items: [{ sku: 'A2' }, { sku: 'B2' }] },
      ],
    };
    const result = getArrayContexts('$.orders[0].items[1].sku', nestedData);
    expect(result[0]?.arrayName).toBe('orders');
    expect(result[0]?.count).toBe(2);
  });

  test('nested arrays - inner context arrayName and count', () => {
    const nestedData = {
      orders: [
        { items: [{ sku: 'A1' }, { sku: 'B1' }] },
        { items: [{ sku: 'A2' }, { sku: 'B2' }] },
      ],
    };
    const result = getArrayContexts('$.orders[0].items[1].sku', nestedData);
    expect(result[1]?.arrayName).toBe('items');
    expect(result[1]?.count).toBe(2);
  });

  test('nested arrays - preview from innermost array', () => {
    const nestedData = {
      orders: [
        { items: [{ sku: 'A1' }, { sku: 'B1' }] },
        { items: [{ sku: 'A2' }, { sku: 'B2' }] },
      ],
    };
    const result = getArrayContexts('$.orders[0].items[1].sku', nestedData);
    // Preview from orders[0].items (the innermost array)
    expect(result[1]?.previewValues).toEqual(['A1', 'B1']);
  });

  test('outer context has empty preview values', () => {
    const nestedData = {
      orders: [
        { items: [{ sku: 'A1' }] },
        { items: [{ sku: 'A2' }] },
      ],
    };
    const result = getArrayContexts('$.orders[0].items[0].sku', nestedData);
    expect(result[0]?.previewValues).toHaveLength(0);
  });
});
