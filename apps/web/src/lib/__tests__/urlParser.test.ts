import { describe, it, expect } from 'vitest';
import { parseRestUrl } from '../urlParser';

describe('parseRestUrl', () => {
  // ── Empty / blank ──────────────────────────────────────────────────────────

  it('returns EMPTY error for empty string', () => {
    const result = parseRestUrl('');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('EMPTY');
    expect(result.pathParams).toHaveLength(0);
    expect(result.queryParams).toHaveLength(0);
  });

  it('returns EMPTY error for whitespace-only string', () => {
    const result = parseRestUrl('   ');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('EMPTY');
  });

  // ── No protocol ───────────────────────────────────────────────────────────

  it('returns NO_PROTOCOL error when scheme is missing', () => {
    const result = parseRestUrl('api.example.com/v1/users');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('NO_PROTOCOL');
  });

  // ── Relative URL ──────────────────────────────────────────────────────────

  it('returns RELATIVE_URL error for path-only URLs', () => {
    const result = parseRestUrl('/v1/users');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('RELATIVE_URL');
  });

  // ── Unsupported protocol ──────────────────────────────────────────────────

  it('returns UNSUPPORTED_PROTOCOL error for ftp scheme', () => {
    const result = parseRestUrl('ftp://example.com/files');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('UNSUPPORTED_PROTOCOL');
  });

  it('returns UNSUPPORTED_PROTOCOL error for ws scheme', () => {
    const result = parseRestUrl('ws://example.com/socket');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('UNSUPPORTED_PROTOCOL');
  });

  // ── Invalid URL ───────────────────────────────────────────────────────────

  it('returns INVALID_URL error for gibberish with a colon-slash-slash', () => {
    const result = parseRestUrl('http://');
    expect(result.isValid).toBe(false);
    expect(result.error).toBe('INVALID_URL');
  });

  // ── Zero parameters ───────────────────────────────────────────────────────

  it('returns valid result with no params for a simple URL', () => {
    const result = parseRestUrl('https://api.example.com/v1/users');
    expect(result.isValid).toBe(true);
    expect(result.error).toBeUndefined();
    expect(result.pathParams).toHaveLength(0);
    expect(result.queryParams).toHaveLength(0);
    expect(result.baseUrl).toBe('https://api.example.com/v1/users');
  });

  it('handles http scheme', () => {
    const result = parseRestUrl('http://localhost:3001/v1/ping');
    expect(result.isValid).toBe(true);
    expect(result.baseUrl).toBe('http://localhost:3001/v1/ping');
  });

  // ── Path params ───────────────────────────────────────────────────────────

  it('detects a single path param', () => {
    const result = parseRestUrl('https://api.example.com/v1/users/{userId}');
    expect(result.isValid).toBe(true);
    expect(result.pathParams).toHaveLength(1);
    expect(result.pathParams[0]).toMatchObject({ name: 'userId', type: 'string', example: '' });
  });

  it('detects multiple path params', () => {
    const result = parseRestUrl(
      'https://api.example.com/v1/orgs/{orgId}/repos/{repoId}/commits/{sha}'
    );
    expect(result.isValid).toBe(true);
    expect(result.pathParams.map((p) => p.name)).toEqual(['orgId', 'repoId', 'sha']);
  });

  it('deduplicates repeated path param names', () => {
    const result = parseRestUrl('https://api.example.com/v1/{id}/children/{id}');
    expect(result.pathParams).toHaveLength(1);
    expect(result.pathParams[0]?.name).toBe('id');
  });

  it('handles path params with underscores and hyphens', () => {
    const result = parseRestUrl('https://api.example.com/v1/{user_id}/posts/{post-slug}');
    expect(result.pathParams.map((p) => p.name)).toEqual(['user_id', 'post-slug']);
  });

  it('does not detect query string values as path params', () => {
    const result = parseRestUrl('https://api.example.com/v1/users?filter={name}');
    // {name} in query string is not a standard path param — URL constructor
    // encodes the braces, they won't match the path-param regex in pathname.
    expect(result.isValid).toBe(true);
    expect(result.pathParams).toHaveLength(0);
  });

  // ── Query params ──────────────────────────────────────────────────────────

  it('detects query params and preserves raw values', () => {
    const result = parseRestUrl('https://api.example.com/v1/users?page=1&limit=20');
    expect(result.isValid).toBe(true);
    expect(result.queryParams).toHaveLength(2);
    expect(result.queryParams[0]).toMatchObject({ key: 'page', rawValue: '1', type: 'number' });
    expect(result.queryParams[1]).toMatchObject({ key: 'limit', rawValue: '20', type: 'number' });
  });

  it('infers boolean type for true/false query values', () => {
    const result = parseRestUrl('https://api.example.com/v1/posts?published=true&draft=false');
    expect(result.queryParams[0]?.type).toBe('boolean');
    expect(result.queryParams[1]?.type).toBe('boolean');
  });

  it('infers string type for non-numeric, non-boolean query values', () => {
    const result = parseRestUrl('https://api.example.com/v1/search?q=hello&sort=asc');
    expect(result.queryParams[0]?.type).toBe('string');
    expect(result.queryParams[1]?.type).toBe('string');
  });

  it('handles query params with empty values', () => {
    const result = parseRestUrl('https://api.example.com/v1/items?filter=');
    expect(result.queryParams).toHaveLength(1);
    expect(result.queryParams[0]).toMatchObject({ key: 'filter', rawValue: '', type: 'string' });
  });

  // ── Combined path + query params ──────────────────────────────────────────

  it('detects both path and query params together', () => {
    const result = parseRestUrl(
      'https://api.example.com/v1/users/{userId}/posts?page=2&published=true'
    );
    expect(result.isValid).toBe(true);
    expect(result.pathParams).toHaveLength(1);
    expect(result.pathParams[0]?.name).toBe('userId');
    expect(result.queryParams).toHaveLength(2);
  });

  // ── baseUrl strips query string ────────────────────────────────────────────

  it('strips query string from baseUrl', () => {
    const result = parseRestUrl('https://api.example.com/v1/users?page=1');
    expect(result.baseUrl).toBe('https://api.example.com/v1/users');
  });

  it('strips fragment from baseUrl', () => {
    const result = parseRestUrl('https://api.example.com/v1/users#section');
    expect(result.baseUrl).toBe('https://api.example.com/v1/users');
  });
});
