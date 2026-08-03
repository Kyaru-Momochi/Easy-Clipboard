import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { createSettingsStore } from './stores/settings';
import type { AppSettings, ThemeMode } from './types';

export type ResolvedTheme = 'light' | 'dark';

export interface MediaQuery {
  readonly matches: boolean;
  addEventListener(
    event: 'change',
    listener: (event: { matches: boolean }) => void
  ): void;
  removeEventListener(
    event: 'change',
    listener: (event: { matches: boolean }) => void
  ): void;
}

interface NativeEvent {
  payload: unknown;
}

type SettingsStore = ReturnType<typeof createSettingsStore>;
type ListenToEvent = (
  event: string,
  listener: (event: NativeEvent) => void
) => Promise<UnlistenFn>;

interface ThemeControllerOptions {
  root?: HTMLElement;
  store: SettingsStore;
  matchMedia?: (query: string) => MediaQuery;
  listenToEvent?: ListenToEvent;
}

export function resolveTheme(
  mode: ThemeMode,
  systemPrefersDark: boolean
): ResolvedTheme {
  return mode === 'system' ? (systemPrefersDark ? 'dark' : 'light') : mode;
}

function defaultMatchMedia(query: string): MediaQuery {
  if (typeof window.matchMedia === 'function') {
    return window.matchMedia(query);
  }
  return {
    matches: false,
    addEventListener: () => undefined,
    removeEventListener: () => undefined
  };
}

const defaultListen: ListenToEvent = (event, listener) =>
  listen<unknown>(event, (nativeEvent) =>
    listener({ payload: nativeEvent.payload })
  );

export function createThemeController({
  root = document.documentElement,
  store,
  matchMedia = defaultMatchMedia,
  listenToEvent = defaultListen
}: ThemeControllerOptions) {
  const darkQuery = matchMedia('(prefers-color-scheme: dark)');
  const reducedQuery = matchMedia('(prefers-reduced-motion: reduce)');
  let settings: AppSettings | null = null;

  function apply(): void {
    if (!settings) {
      return;
    }
    root.dataset.theme = resolveTheme(settings.theme, darkQuery.matches);
    root.style.setProperty('--motion-scale', String(settings.motionScale));
    root.classList.toggle('reduce-motion', reducedQuery.matches);
  }

  function handleSystemChange(): void {
    apply();
  }

  return {
    mount(): () => void {
      let disposed = false;
      let unlisten: UnlistenFn | null = null;
      const unsubscribe = store.subscribe((state) => {
        settings = state.value;
        apply();
      });

      darkQuery.addEventListener('change', handleSystemChange);
      reducedQuery.addEventListener('change', handleSystemChange);
      void store.ensureLoaded().catch(() => undefined);
      void listenToEvent('settings-changed', (event) => {
        store.receive(event.payload);
      })
        .then((listener) => {
          if (disposed) {
            listener();
          } else {
            unlisten = listener;
            // The native listener was not active while the initial load was in
            // flight. Reconcile once after registration so a remote save in that
            // gap cannot leave this webview stale.
            void store.load().catch(() => undefined);
          }
        })
        .catch(() => undefined);

      return () => {
        disposed = true;
        unsubscribe();
        darkQuery.removeEventListener('change', handleSystemChange);
        reducedQuery.removeEventListener('change', handleSystemChange);
        unlisten?.();
      };
    }
  };
}
