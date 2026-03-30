import { describe, it, expect } from 'vitest';

describe('sample', () => {
  it('adds two numbers', () => {
    expect(1 + 1).toBe(2);
  });

  it('concats strings', () => {
    expect('hello' + ' ' + 'world').toBe('hello world');
  });

  it('array includes element', () => {
    const arr = [1, 2, 3];
    expect(arr).toContain(2);
  });
});
