import { invoke } from '@tauri-apps/api/core';

import type { AppSettings, ClipboardItem, HistoryQuery, ItemId } from './types';

function assertSafeInteger(value: number, field: string, nonNegative = false): void {
  if (!Number.isSafeInteger(value) || (nonNegative && value < 0)) {
    throw new RangeError(`${field} is outside JavaScript's safe integer range`);
  }
}

function validateItemNumbers(item: ClipboardItem): void {
  assertSafeInteger(item.byteSize, 'ClipboardItem.byteSize', true);
  assertSafeInteger(item.createdAtMs, 'ClipboardItem.createdAtMs');
  assertSafeInteger(item.updatedAtMs, 'ClipboardItem.updatedAtMs');

  if (item.payload.type === 'image') {
    assertSafeInteger(item.payload.width, 'ImagePayload.width', true);
    assertSafeInteger(item.payload.height, 'ImagePayload.height', true);
  } else if (item.payload.type === 'files') {
    for (const entry of item.payload.entries) {
      assertSafeInteger(entry.sizeBytes, 'FileEntry.sizeBytes', true);
    }
  }
}

function validateSettingsNumbers(settings: AppSettings): void {
  assertSafeInteger(settings.historyLimit, 'AppSettings.historyLimit', true);
  assertSafeInteger(settings.favoriteLimit, 'AppSettings.favoriteLimit', true);
  assertSafeInteger(settings.maxItemBytes, 'AppSettings.maxItemBytes', true);
}

export async function listHistory(query: HistoryQuery): Promise<ClipboardItem[]> {
  const items = await invoke<ClipboardItem[]>('list_history', { query });
  items.forEach(validateItemNumbers);
  return items;
}

export function pasteItem(id: ItemId): Promise<void> {
  return invoke<void>('paste_item', { id });
}

export function copyItem(id: ItemId): Promise<void> {
  return invoke<void>('copy_item', { id });
}

export function setFavorite(id: ItemId, value: boolean): Promise<void> {
  return invoke<void>('set_favorite', { id, value });
}

export function deleteItem(id: ItemId): Promise<void> {
  return invoke<void>('delete_item', { id });
}

export function clearHistory(): Promise<void> {
  return invoke<void>('clear_history');
}

export async function getSettings(): Promise<AppSettings> {
  const settings = await invoke<AppSettings>('get_settings');
  validateSettingsNumbers(settings);
  return settings;
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  validateSettingsNumbers(settings);
  await invoke<void>('save_settings', { settings });
}

export function openSettings(): Promise<void> {
  return invoke<void>('open_settings');
}

export function revealFile(path: string): Promise<void> {
  return invoke<void>('reveal_file', { path });
}

export function exitApp(): Promise<void> {
  return invoke<void>('exit_app');
}
