export type ItemId = string;

export type ClipboardKind = 'text' | 'image' | 'files';
export type ThemeMode = 'system' | 'light' | 'dark';
export type MediaKind = 'audio' | 'video' | 'other';

export interface FileEntry {
  path: string;
  name: string;
  extension: string;
  sizeBytes: number;
  mediaKind: MediaKind;
  available: boolean;
  availabilityPending?: boolean;
}

export interface TextPayload {
  type: 'text';
  plain: string;
  html: string | null;
  rtf: number[] | null;
}

export interface ImagePayload {
  type: 'image';
  pngPath: string;
  thumbnailPath: string;
  width: number;
  height: number;
}

export interface FilesPayload {
  type: 'files';
  entries: FileEntry[];
}

export type ClipboardPayload = TextPayload | ImagePayload | FilesPayload;

export interface ClipboardItem {
  id: ItemId;
  kind: ClipboardKind;
  payload: ClipboardPayload;
  fingerprint: string;
  preview: string;
  byteSize: number;
  isFavorite: boolean;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface HistoryQuery {
  kind: ClipboardKind | null;
  search: string;
}

export interface HistoryFilter {
  kind: ClipboardKind | 'all';
  search: string;
}

export interface AppSettings {
  historyLimit: number;
  favoriteLimit: number;
  maxItemBytes: number;
  hotkey: string;
  theme: ThemeMode;
  motionScale: number;
  autostart: boolean;
  clearOnExit: boolean;
}

export interface AppInfo {
  version: string;
  appDataDir: string;
}

export interface AppError {
  code: string;
  message: string;
}
