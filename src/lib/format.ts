import type { MediaKind } from './types';

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;
const AUDIO_EXTENSIONS = new Set([
  'aac',
  'aiff',
  'alac',
  'flac',
  'm4a',
  'mp3',
  'ogg',
  'opus',
  'wav',
  'wma'
]);
const VIDEO_EXTENSIONS = new Set([
  'avi',
  'flv',
  'm4v',
  'mkv',
  'mov',
  'mp4',
  'mpeg',
  'mpg',
  'webm',
  'wmv'
]);

export function formatBytes(bytes: number): string {
  if (!Number.isSafeInteger(bytes) || bytes < 0) {
    throw new RangeError('bytes must be a safe, non-negative integer');
  }
  if (bytes === 0) {
    return '0 B';
  }

  const unitIndex = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    BYTE_UNITS.length - 1
  );
  const value = bytes / 1024 ** unitIndex;
  const formatted = value >= 10 || Number.isInteger(value) ? value.toFixed(0) : value.toFixed(1);
  return `${formatted} ${BYTE_UNITS[unitIndex]}`;
}

export function formatRelativeTime(timestampMs: number, nowMs = Date.now()): string {
  if (!Number.isFinite(timestampMs) || !Number.isFinite(nowMs)) {
    throw new RangeError('timestamps must be finite numbers');
  }

  const elapsedMs = Math.max(0, nowMs - timestampMs);
  if (elapsedMs < 60_000) {
    return 'just now';
  }
  if (elapsedMs < 60 * 60_000) {
    return `${Math.floor(elapsedMs / 60_000)}m ago`;
  }
  if (elapsedMs < 24 * 60 * 60_000) {
    return `${Math.floor(elapsedMs / (60 * 60_000))}h ago`;
  }
  if (elapsedMs < 7 * 24 * 60 * 60_000) {
    return `${Math.floor(elapsedMs / (24 * 60 * 60_000))}d ago`;
  }

  const date = new Date(timestampMs);
  if (Number.isNaN(date.getTime())) {
    throw new RangeError('timestamp is outside the supported date range');
  }
  return date.toISOString().slice(0, 10);
}

export function classifyExtension(filename: string): MediaKind {
  const lastSeparator = Math.max(filename.lastIndexOf('/'), filename.lastIndexOf('\\'));
  const lastDot = filename.lastIndexOf('.');
  if (lastDot <= lastSeparator || lastDot === filename.length - 1) {
    return 'other';
  }

  const extension = filename.slice(lastDot + 1).toLowerCase();
  if (AUDIO_EXTENSIONS.has(extension)) {
    return 'audio';
  }
  if (VIDEO_EXTENSIONS.has(extension)) {
    return 'video';
  }
  return 'other';
}
