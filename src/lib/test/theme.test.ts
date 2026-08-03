// @vitest-environment jsdom

import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import {
  createThemeController,
  resolveTheme,
  type MediaQuery
} from '../theme';
import { createSettingsStore } from '../stores/settings';
import type { AppSettings } from '../types';

const settings: AppSettings = {
  historyLimit: 100,
  favoriteLimit: 20,
  maxItemBytes: 50 * 1024 * 1024,
  hotkey: 'Ctrl+Shift+V',
  theme: 'system',
  motionScale: 1,
  autostart: false,
  clearOnExit: false
};

function mediaQuery(initial: boolean): MediaQuery & {
  setMatches(value: boolean): void;
  listenerCount(): number;
} {
  let matches = initial;
  const listeners = new Set<(event: { matches: boolean }) => void>();
  return {
    get matches() {
      return matches;
    },
    addEventListener: vi.fn((_event, listener) => listeners.add(listener)),
    removeEventListener: vi.fn((_event, listener) => listeners.delete(listener)),
    setMatches(value) {
      matches = value;
      for (const listener of listeners) {
        listener({ matches: value });
      }
    },
    listenerCount: () => listeners.size
  };
}

describe('theme resolution', () => {
  it('resolves explicit light and dark without consulting the system preference', () => {
    expect(resolveTheme('light', true)).toBe('light');
    expect(resolveTheme('dark', false)).toBe('dark');
  });

  it('resolves system mode from the current system preference', () => {
    expect(resolveTheme('system', false)).toBe('light');
    expect(resolveTheme('system', true)).toBe('dark');
  });
});

describe('theme controller', () => {
  it('reacts to system theme, reduced motion, settings changes, and cleans up late listeners', async () => {
    const dark = mediaQuery(false);
    const reduced = mediaQuery(false);
    const listeners = new Map<string, (event: { payload: unknown }) => void>();
    const unlisten = vi.fn();
    const store = createSettingsStore({
      getSettings: vi.fn(async () => settings),
      saveSettings: vi.fn(async () => undefined)
    });
    const root = document.createElement('div');
    const controller = createThemeController({
      root,
      store,
      matchMedia: (query) =>
        query.includes('prefers-color-scheme') ? dark : reduced,
      listenToEvent: vi.fn(async (event, listener) => {
        listeners.set(event, listener);
        return unlisten;
      })
    });

    const dispose = controller.mount();
    await vi.waitFor(() => expect(get(store).value).toEqual(settings));
    await vi.waitFor(() => expect(listeners.has('settings-changed')).toBe(true));
    expect(root.dataset.theme).toBe('light');
    expect(root.style.getPropertyValue('--motion-scale')).toBe('1');

    dark.setMatches(true);
    expect(root.dataset.theme).toBe('dark');

    reduced.setMatches(true);
    expect(root.classList.contains('reduce-motion')).toBe(true);

    listeners.get('settings-changed')?.({
      payload: { ...settings, theme: 'light', motionScale: 0.65 }
    });
    expect(root.dataset.theme).toBe('light');
    expect(root.style.getPropertyValue('--motion-scale')).toBe('0.65');
    expect(get(store).value?.theme).toBe('light');

    dispose();
    expect(dark.listenerCount()).toBe(0);
    expect(reduced.listenerCount()).toBe(0);
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it('removes a native listener that resolves after disposal', async () => {
    const dark = mediaQuery(false);
    const reduced = mediaQuery(false);
    let resolveListen!: (unlisten: () => void) => void;
    const lateUnlisten = vi.fn();
    const store = createSettingsStore({
      getSettings: vi.fn(async () => settings),
      saveSettings: vi.fn(async () => undefined)
    });
    const controller = createThemeController({
      root: document.createElement('div'),
      store,
      matchMedia: (query) =>
        query.includes('prefers-color-scheme') ? dark : reduced,
      listenToEvent: vi.fn(
        () =>
          new Promise<() => void>((resolve) => {
            resolveListen = resolve;
          })
      )
    });

    const dispose = controller.mount();
    dispose();
    resolveListen(lateUnlisten);

    await vi.waitFor(() => expect(lateUnlisten).toHaveBeenCalledOnce());
  });

  it('reconciles authoritative settings after native listener registration closes the startup gap', async () => {
    const dark = mediaQuery(false);
    const reduced = mediaQuery(false);
    const oldSettings = { ...settings, theme: 'light' as const };
    const latestSettings = {
      ...settings,
      theme: 'dark' as const,
      motionScale: 0.7
    };
    const backend = {
      getSettings: vi
        .fn()
        .mockResolvedValueOnce(oldSettings)
        .mockResolvedValueOnce(latestSettings),
      saveSettings: vi.fn(async () => undefined)
    };
    const store = createSettingsStore(backend);
    let resolveListen!: (unlisten: () => void) => void;
    const root = document.createElement('div');
    const controller = createThemeController({
      root,
      store,
      matchMedia: (query) =>
        query.includes('prefers-color-scheme') ? dark : reduced,
      listenToEvent: vi.fn(
        () =>
          new Promise<() => void>((resolve) => {
            resolveListen = resolve;
          })
      )
    });

    const dispose = controller.mount();
    await vi.waitFor(() => expect(root.dataset.theme).toBe('light'));
    expect(backend.getSettings).toHaveBeenCalledOnce();

    // A remote save happens before the native listener promise settles, so no
    // event callback can be delivered to this webview.
    resolveListen(vi.fn());

    await vi.waitFor(() => expect(backend.getSettings).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(root.dataset.theme).toBe('dark'));
    expect(root.style.getPropertyValue('--motion-scale')).toBe('0.7');
    expect(get(store).value).toEqual(latestSettings);
    dispose();
  });

  it('keeps initial settings when native listener registration rejects', async () => {
    const dark = mediaQuery(false);
    const reduced = mediaQuery(false);
    const backend = {
      getSettings: vi.fn(async () => settings),
      saveSettings: vi.fn(async () => undefined)
    };
    const store = createSettingsStore(backend);
    const root = document.createElement('div');
    const controller = createThemeController({
      root,
      store,
      matchMedia: (query) =>
        query.includes('prefers-color-scheme') ? dark : reduced,
      listenToEvent: vi.fn(async () => {
        throw new Error('event bridge unavailable');
      })
    });

    const dispose = controller.mount();

    await vi.waitFor(() => expect(root.dataset.theme).toBe('light'));
    expect(backend.getSettings).toHaveBeenCalledOnce();
    dispose();
  });

  it('does not let reconciliation overwrite a newer settings event', async () => {
    const dark = mediaQuery(false);
    const reduced = mediaQuery(false);
    let resolveReconciliation!: (value: AppSettings) => void;
    const reconciliation = new Promise<AppSettings>((resolve) => {
      resolveReconciliation = resolve;
    });
    const backend = {
      getSettings: vi
        .fn()
        .mockResolvedValueOnce({ ...settings, theme: 'light' as const })
        .mockImplementationOnce(() => reconciliation),
      saveSettings: vi.fn(async () => undefined)
    };
    const store = createSettingsStore(backend);
    let settingsChanged!: (event: { payload: unknown }) => void;
    const root = document.createElement('div');
    const controller = createThemeController({
      root,
      store,
      matchMedia: (query) =>
        query.includes('prefers-color-scheme') ? dark : reduced,
      listenToEvent: vi.fn(async (_event, listener) => {
        settingsChanged = listener;
        return vi.fn();
      })
    });

    const dispose = controller.mount();
    await vi.waitFor(() => expect(backend.getSettings).toHaveBeenCalledTimes(2));
    const remote = { ...settings, theme: 'dark' as const, motionScale: 0.55 };
    settingsChanged({ payload: remote });
    expect(root.dataset.theme).toBe('dark');

    resolveReconciliation({ ...settings, theme: 'light' });
    await reconciliation;
    await Promise.resolve();

    expect(root.dataset.theme).toBe('dark');
    expect(root.style.getPropertyValue('--motion-scale')).toBe('0.55');
    expect(get(store).value).toEqual(remote);
    dispose();
  });
});
