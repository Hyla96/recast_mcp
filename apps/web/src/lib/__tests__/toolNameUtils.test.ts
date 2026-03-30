import { describe, it, expect } from 'vitest';
import { generateToolName, validateToolName, filterToolNameChars } from '../toolNameUtils';

// ─── generateToolName ─────────────────────────────────────────────────────────

describe('generateToolName', () => {
  it('returns my_tool for an empty string', () => {
    expect(generateToolName('')).toBe('my_tool');
  });

  it('returns my_tool for a whitespace-only string', () => {
    expect(generateToolName('   ')).toBe('my_tool');
  });

  it('returns my_tool for an invalid URL', () => {
    expect(generateToolName('not-a-url')).toBe('my_tool');
  });

  it('returns my_tool for a root path URL', () => {
    expect(generateToolName('https://api.example.com/')).toBe('my_tool');
  });

  it('derives name from a simple path', () => {
    expect(generateToolName('https://api.example.com/users')).toBe('users');
  });

  it('joins multiple path segments with underscores', () => {
    expect(generateToolName('https://api.example.com/v1/users')).toBe('v1_users');
  });

  it('strips path param placeholders', () => {
    expect(generateToolName('https://api.example.com/users/{userId}/posts')).toBe('users_posts');
  });

  it('falls back to my_tool when only path params remain', () => {
    expect(generateToolName('https://api.example.com/{id}')).toBe('my_tool');
  });

  it('lowercases uppercase path segments', () => {
    expect(generateToolName('https://api.example.com/MyResource/SubPath')).toBe(
      'myresource_subpath'
    );
  });

  it('replaces hyphens and dots with underscores', () => {
    expect(generateToolName('https://api.example.com/my-resource')).toBe('my_resource');
  });

  it('collapses consecutive underscores', () => {
    expect(generateToolName('https://api.example.com/my--path')).toBe('my_path');
  });

  it('strips leading and trailing underscores', () => {
    // URL-encoded leading underscore in segment
    expect(generateToolName('https://api.example.com/_internal_')).toBe('internal');
  });

  it('truncates to 50 characters', () => {
    const longPath = 'a'.repeat(60);
    const result = generateToolName(`https://api.example.com/${longPath}`);
    expect(result.length).toBeLessThanOrEqual(50);
  });

  it('returns my_tool when truncated result is fewer than 3 chars', () => {
    expect(generateToolName('https://api.example.com/ab')).toBe('my_tool');
  });

  it('handles query strings without including them in the name', () => {
    expect(generateToolName('https://api.example.com/search?q=hello')).toBe('search');
  });
});

// ─── validateToolName ─────────────────────────────────────────────────────────

describe('validateToolName', () => {
  it('returns null for a valid name', () => {
    expect(validateToolName('get_users')).toBeNull();
  });

  it('returns error for names shorter than 3 chars', () => {
    expect(validateToolName('ab')).not.toBeNull();
    expect(validateToolName('')).not.toBeNull();
  });

  it('returns error for names longer than 50 chars', () => {
    expect(validateToolName('a'.repeat(51))).not.toBeNull();
  });

  it('accepts names of exactly 3 and 50 chars', () => {
    expect(validateToolName('abc')).toBeNull();
    expect(validateToolName('a'.repeat(50))).toBeNull();
  });

  it('rejects uppercase letters', () => {
    expect(validateToolName('GetUsers')).not.toBeNull();
  });

  it('rejects hyphens', () => {
    expect(validateToolName('get-users')).not.toBeNull();
  });

  it('rejects names starting with underscore', () => {
    expect(validateToolName('_get_users')).not.toBeNull();
  });

  it('rejects names ending with underscore', () => {
    expect(validateToolName('get_users_')).not.toBeNull();
  });

  it('rejects names with consecutive underscores', () => {
    expect(validateToolName('get__users')).not.toBeNull();
  });

  it('accepts names with numbers', () => {
    expect(validateToolName('get_users_v2')).toBeNull();
  });

  it('accepts names starting with a digit', () => {
    expect(validateToolName('v2_users')).toBeNull();
  });
});

// ─── filterToolNameChars ──────────────────────────────────────────────────────

describe('filterToolNameChars', () => {
  it('passes through valid characters unchanged', () => {
    expect(filterToolNameChars('hello_world123')).toBe('hello_world123');
  });

  it('silently removes uppercase letters', () => {
    expect(filterToolNameChars('Hello')).toBe('ello');
  });

  it('silently removes hyphens', () => {
    expect(filterToolNameChars('get-users')).toBe('getusers');
  });

  it('silently removes spaces', () => {
    expect(filterToolNameChars('get users')).toBe('getusers');
  });

  it('returns empty string for fully invalid input', () => {
    expect(filterToolNameChars('ABC!@#')).toBe('');
  });
});
