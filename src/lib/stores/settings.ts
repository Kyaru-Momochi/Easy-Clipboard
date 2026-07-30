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
    !Number.isSafeInteger(settings.maxItemBytes) ||
    settings.maxItemBytes < BYTES_PER_MEGABYTE ||
    settings.maxItemBytes > MAX_ITEM_BYTES
  ) {
    errors.maxItemBytes = 'Item limit must be from 1 to 500 MB';
  }
  if (!Number.isFinite(settings.motionScale)) {
    errors.motionScale = 'Motion scale must be finite';
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
  let pendingLoads = 0;
  let pendingSaves = 0;
  let saveQueue = Promise.resolve();

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

  return { subscribe, load, save };
}

export const settingsStore = createSettingsStore();
