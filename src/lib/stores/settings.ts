import { writable } from 'svelte/store';

import * as api from '../api';
import type { AppSettings } from '../types';

const BYTES_PER_MEGABYTE = 1024 * 1024;
const MAX_ITEM_BYTES = 500 * BYTES_PER_MEGABYTE;

export type SettingsValidationErrors = Partial<Record<keyof AppSettings, string>>;

export interface SettingsState {
  value: AppSettings | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  validationErrors: SettingsValidationErrors;
}

export interface SettingsApi {
  getSettings(): Promise<AppSettings>;
  saveSettings(settings: AppSettings): Promise<void>;
}

export function validateSettings(settings: AppSettings): SettingsValidationErrors {
  const errors: SettingsValidationErrors = {};
  if (
    !Number.isSafeInteger(settings.historyLimit) ||
    settings.historyLimit < 0
  ) {
    errors.historyLimit = '历史数量必须是非负整数';
  }
  if (
    !Number.isSafeInteger(settings.favoriteLimit) ||
    settings.favoriteLimit < 0
  ) {
    errors.favoriteLimit = '收藏数量必须是非负整数';
  }
  if (typeof settings.hotkey !== 'string' || settings.hotkey.trim() === '') {
    errors.hotkey = '请输入全局热键';
  }
  if (
    typeof settings.theme !== 'string' ||
    !['system', 'light', 'dark'].includes(settings.theme)
  ) {
    errors.theme = '请选择有效主题';
  }
  if (
    !Number.isSafeInteger(settings.maxItemBytes) ||
    settings.maxItemBytes < BYTES_PER_MEGABYTE ||
    settings.maxItemBytes > MAX_ITEM_BYTES
  ) {
    errors.maxItemBytes = 'Item limit must be from 1 to 500 MB';
  }
  if (
    typeof settings.motionScale !== 'number' ||
    !Number.isFinite(settings.motionScale) ||
    settings.motionScale < 0 ||
    settings.motionScale > 2
  ) {
    errors.motionScale = '动画强度必须在 0–2 之间';
  }
  if (typeof settings.autostart !== 'boolean') {
    errors.autostart = '开机启动设置无效';
  }
  if (typeof settings.clearOnExit !== 'boolean') {
    errors.clearOnExit = '退出清理设置无效';
  }
  return errors;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (
    typeof error === 'object' &&
    error !== null &&
    'message' in error &&
    typeof error.message === 'string'
  ) {
    return error.message;
  }
  return String(error);
}

export function createSettingsStore(backend: SettingsApi = api) {
  const { subscribe, update } = writable<SettingsState>({
    value: null,
    loading: false,
    saving: false,
    error: null,
    validationErrors: {}
  });
  let generation = 0;
  let remoteGeneration = 0;
  let pendingLoads = 0;
  let pendingSaves = 0;
  let saveQueue = Promise.resolve();
  let ensured = false;
  let ensurePromise: Promise<void> | null = null;

  async function load(): Promise<void> {
    const operationGeneration = ++generation;
    pendingLoads += 1;
    update((state) => ({
      ...state,
      loading: true,
      error: null,
      validationErrors: {}
    }));
    const priorSaves = saveQueue;
    try {
      await priorSaves;
      const value = { ...(await backend.getSettings()) };
      if (operationGeneration === generation) {
        update((state) => ({
          ...state,
          value,
          error: null,
          validationErrors: {}
        }));
      }
    } catch (error) {
      if (operationGeneration === generation) {
        update((state) => ({ ...state, error: errorMessage(error) }));
      }
      throw error;
    } finally {
      pendingLoads -= 1;
      update((state) => ({ ...state, loading: pendingLoads > 0 }));
    }
  }

  async function save(settings: AppSettings): Promise<void> {
    const snapshot = { ...settings };
    const operationGeneration = ++generation;
    const operationRemoteGeneration = remoteGeneration;
    const validationErrors = validateSettings(snapshot);
    if (Object.keys(validationErrors).length > 0) {
      update((state) => ({ ...state, validationErrors, error: null }));
      return;
    }

    pendingSaves += 1;
    update((state) => ({
      ...state,
      saving: true,
      error: null,
      validationErrors: {}
    }));
    const backendSnapshot = { ...snapshot };
    const operation = saveQueue.then(() => backend.saveSettings(backendSnapshot));
    saveQueue = operation.then(
      () => undefined,
      () => undefined
    );
    try {
      await operation;
      if (operationRemoteGeneration === remoteGeneration) {
        update((state) =>
          operationGeneration === generation
            ? {
                ...state,
                value: { ...snapshot },
                error: null,
                validationErrors: {}
              }
            : {
                ...state,
                value: { ...snapshot }
              }
        );
      }
    } catch (error) {
      if (operationGeneration === generation) {
        update((state) => ({ ...state, error: errorMessage(error) }));
      }
      throw error;
    } finally {
      pendingSaves -= 1;
      update((state) => ({ ...state, saving: pendingSaves > 0 }));
    }
  }

  function ensureLoaded(): Promise<void> {
    if (ensured) {
      return Promise.resolve();
    }
    if (ensurePromise === null) {
      ensurePromise = load()
        .then(() => {
          ensured = true;
        })
        .finally(() => {
          ensurePromise = null;
        });
    }
    return ensurePromise;
  }

  function receive(value: unknown): boolean {
    if (typeof value !== 'object' || value === null) {
      return false;
    }
    const snapshot = { ...(value as AppSettings) };
    if (Object.keys(validateSettings(snapshot)).length > 0) {
      return false;
    }
    generation += 1;
    remoteGeneration += 1;
    update((state) => ({
      ...state,
      value: snapshot,
      error: null,
      validationErrors: {}
    }));
    return true;
  }

  return { subscribe, load, ensureLoaded, save, receive };
}

export const settingsStore = createSettingsStore();
