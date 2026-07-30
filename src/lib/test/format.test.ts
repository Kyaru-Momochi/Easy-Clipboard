import { describe, expect, it } from 'vitest';

import { classifyExtension, formatBytes, formatRelativeTime } from '../format';

describe('formatBytes', () => {
  it('formats zero bytes', () => {
    expect(formatBytes(0)).toBe('0 B');
  });

  it('uses binary units with one useful decimal place', () => {
    expect(formatBytes(1_572_864)).toBe('1.5 MB');
  });

  it.each([
    -1,
    0.5,
    Number.MAX_SAFE_INTEGER + 1,
    Number.NaN,
    Number.POSITIVE_INFINITY
  ])(
    'rejects invalid byte counts (%s)',
    (bytes) => {
      expect(() => formatBytes(bytes)).toThrow(RangeError);
    }
  );
});

describe('formatRelativeTime', () => {
  const now = Date.UTC(2026, 6, 30, 12);

  it('formats recent timestamps with compact relative labels', () => {
    expect(formatRelativeTime(now - 30_000, now)).toBe('just now');
    expect(formatRelativeTime(now - 90_000, now)).toBe('1m ago');
    expect(formatRelativeTime(now - 2 * 60 * 60_000, now)).toBe('2h ago');
    expect(formatRelativeTime(now - 3 * 24 * 60 * 60_000, now)).toBe('3d ago');
  });

  it('uses a stable calendar date for older timestamps', () => {
    expect(formatRelativeTime(Date.UTC(2026, 5, 1), now)).toBe('2026-06-01');
  });

  it('treats a future timestamp as just now', () => {
    expect(formatRelativeTime(now + 5_000, now)).toBe('just now');
  });
});

describe('classifyExtension', () => {
  it('classifies extensions case-insensitively', () => {
    expect(classifyExtension('mix.FLAC')).toBe('audio');
    expect(classifyExtension('clip.WebM')).toBe('video');
  });

  it('matches every additional Rust media extension', () => {
    expect(classifyExtension('track.aiff')).toBe('audio');
    expect(classifyExtension('lossless.ALAC')).toBe('audio');
    expect(classifyExtension('legacy.flv')).toBe('video');
  });

  it('returns other when a file has no extension', () => {
    expect(classifyExtension('README')).toBe('other');
  });
});
