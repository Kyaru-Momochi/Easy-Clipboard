import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import * as clipboardApi from '../api';
import { createHistoryStore, filterItems, moveSelection } from '../stores/history';
import { createSettingsStore, validateSettings } from '../stores/settings';
import type {
  AppSettings,
  ClipboardItem,
  HistoryQuery,
  ItemId
} from '../types';

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn()
}));

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string) => path),
  invoke: invokeMock
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const fixtures: ClipboardItem[] = [
  {
    id: 'text-1',
    kind: 'text',
    payload: {
      type: 'text',
      plain: 'Hello CAFÉ',
      html: null,
      rtf: null
    },
    fingerprint: 'text-fingerprint',
    preview: 'Hello CAFÉ',
    byteSize: 11,
    isFavorite: false,
    createdAtMs: 1_000,
    updatedAtMs: 1_000
  },
  {
    id: 'image-1',
    kind: 'image',
    payload: {
      type: 'image',
      pngPath: 'image.png',
      thumbnailPath: 'thumbnail.png',
      width: 320,
      height: 180
    },
    fingerprint: 'image-fingerprint',
    preview: 'Screenshot',
    byteSize: 1_024,
    isFavorite: true,
    createdAtMs: 2_000,
    updatedAtMs: 2_000
  },
  {
    id: 'files-1',
    kind: 'files',
    payload: {
      type: 'files',
      entries: [
        {
          path: 'C:\\music\\mix.flac',
          name: 'mix.flac',
          extension: 'flac',
          sizeBytes: 2_048,
          mediaKind: 'audio',
          available: true
        }
      ]
    },
    fingerprint: 'files-fingerprint',
    preview: 'mix.flac',
    byteSize: 2_048,
    isFavorite: false,
    createdAtMs: 3_000,
    updatedAtMs: 3_000
  }
];

describe('filterItems', () => {
  it('filters by clipboard kind', () => {
    expect(filterItems(fixtures, { kind: 'image', search: '' })).toHaveLength(1);
  });

  it('searches previews case-insensitively', () => {
    expect(
      filterItems(fixtures, { kind: 'all', search: 'hello' }).map((item) => item.id)
    ).toEqual(['text-1']);
  });

  it('handles Unicode case folding', () => {
    expect(
      filterItems(fixtures, { kind: 'all', search: 'café' }).map((item) => item.id)
    ).toEqual(['text-1']);
  });
});

describe('moveSelection', () => {
  const ids = fixtures.map((item) => item.id);

  it('returns null for an empty list', () => {
    expect(moveSelection([], null, 1)).toBeNull();
  });

  it('clamps movement at the first and last item', () => {
    expect(moveSelection(ids, 'text-1', -1)).toBe('text-1');
    expect(moveSelection(ids, 'files-1', 1)).toBe('files-1');
  });

  it('recovers when the current id disappeared', () => {
    expect(moveSelection(ids, 'deleted-id', 1)).toBe('text-1');
    expect(moveSelection(ids, 'deleted-id', -1)).toBe('files-1');
  });
});

describe('typed command API', () => {
  it('uses the exact Tauri command names and argument keys', async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === 'list_history') {
        return [];
      }
      if (command === 'get_settings') {
        return validSettings;
      }
      return undefined;
    });

    await clipboardApi.listHistory({ kind: 'text', search: 'needle' });
    await clipboardApi.pasteItem('item-1');
    await clipboardApi.copyItem('item-1');
    await clipboardApi.setFavorite('item-1', true);
    await clipboardApi.deleteItem('item-1');
    await clipboardApi.clearHistory();
    await clipboardApi.getSettings();
    await clipboardApi.saveSettings(validSettings);
    await clipboardApi.getAppInfo();
    await clipboardApi.openSettings();
    await clipboardApi.revealFile('C:\\file.txt');
    await clipboardApi.hideOverlay();
    await clipboardApi.exitApp();

    expect(invokeMock.mock.calls).toEqual([
      ['list_history', { query: { kind: 'text', search: 'needle' } }],
      ['paste_item', { id: 'item-1' }],
      ['copy_item', { id: 'item-1' }],
      ['set_favorite', { id: 'item-1', value: true }],
      ['delete_item', { id: 'item-1' }],
      ['clear_history'],
      ['get_settings'],
      ['save_settings', { settings: validSettings }],
      ['get_app_info'],
      ['open_settings'],
      ['reveal_file', { path: 'C:\\file.txt' }],
      ['hide_overlay'],
      ['exit_app']
    ]);
  });

  it('rejects unsafe settings integers before invoking Rust', async () => {
    invokeMock.mockClear();

    await expect(
      clipboardApi.saveSettings({
        ...validSettings,
        maxItemBytes: Number.MAX_SAFE_INTEGER + 1
      })
    ).rejects.toThrow(RangeError);

    expect(invokeMock).not.toHaveBeenCalled();
  });
});

function historyApi(items: ClipboardItem[] = fixtures) {
  let canonicalItems = [...items];
  return {
    listHistory: vi.fn(async (_query: HistoryQuery) => canonicalItems),
    pasteItem: vi.fn(async (_id: ItemId) => undefined),
    copyItem: vi.fn(async (_id: ItemId) => undefined),
    setFavorite: vi.fn(async (id: ItemId, value: boolean) => {
      canonicalItems = canonicalItems
        .map((item) => (item.id === id ? { ...item, isFavorite: value } : item))
        .sort((left, right) => {
          if (left.isFavorite !== right.isFavorite) {
            return left.isFavorite ? -1 : 1;
          }
          if (left.updatedAtMs !== right.updatedAtMs) {
            return left.updatedAtMs > right.updatedAtMs ? -1 : 1;
          }
          return left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
        });
    }),
    deleteItem: vi.fn(async (id: ItemId) => {
      canonicalItems = canonicalItems.filter((item) => item.id !== id);
    }),
    clearHistory: vi.fn(async () => {
      canonicalItems = canonicalItems.filter((item) => item.isFavorite);
    })
  };
}

describe('createHistoryStore', () => {
  it('refreshes through the API and selects the first visible item', async () => {
    const api = historyApi();
    const store = createHistoryStore(api);

    await store.refresh();

    expect(api.listHistory).toHaveBeenCalledWith({ kind: null, search: '' });
    expect(get(store)).toMatchObject({
      items: fixtures,
      visibleItems: fixtures,
      selectedId: 'text-1',
      loading: false,
      error: null
    });
  });

  it('reconciles selection when filtering removes the current item', async () => {
    const api = historyApi([fixtures[1]]);
    const store = createHistoryStore(api);
    await store.refresh();

    await store.setQuery({ kind: 'image', search: '' });

    expect(api.listHistory).toHaveBeenLastCalledWith({ kind: 'image', search: '' });
    expect(get(store).visibleItems.map((item) => item.id)).toEqual(['image-1']);
    expect(get(store).selectedId).toBe('image-1');
  });

  it('serializes refreshes and coalesces a burst into one trailing latest-query request', async () => {
    const first = deferred<ClipboardItem[]>();
    const second = deferred<ClipboardItem[]>();
    const api = historyApi();
    let active = 0;
    let maxActive = 0;
    let call = 0;
    api.listHistory.mockImplementation(async () => {
      active += 1;
      maxActive = Math.max(maxActive, active);
      const response = call++ === 0 ? first : second;
      try {
        return await response.promise;
      } finally {
        active -= 1;
      }
    });
    const store = createHistoryStore(api);

    const imageQuery = store.setQuery({ kind: 'image', search: 'old' });
    const refreshA = store.refresh();
    const filesQuery = store.setQuery({ kind: 'files', search: '' });
    const refreshB = store.refresh();

    expect(api.listHistory).toHaveBeenCalledTimes(1);
    expect(maxActive).toBe(1);

    first.resolve([fixtures[0]]);
    await vi.waitFor(() => expect(api.listHistory).toHaveBeenCalledTimes(2));

    expect(maxActive).toBe(1);
    expect(api.listHistory.mock.calls).toEqual([
      [{ kind: 'image', search: 'old' }],
      [{ kind: 'files', search: '' }]
    ]);
    expect(get(store).items).toEqual([]);
    expect(get(store).loading).toBe(true);

    second.resolve([fixtures[2]]);
    await Promise.all([imageQuery, refreshA, filesQuery, refreshB]);

    expect(api.listHistory).toHaveBeenCalledTimes(2);
    expect(maxActive).toBe(1);
    expect(get(store).visibleItems.map((item) => item.id)).toEqual(['files-1']);
    expect(get(store).selectedId).toBe('files-1');
    expect(get(store).loading).toBe(false);
    expect(get(store).error).toBeNull();
  });

  it('coalesces repeated same-query refreshes while keeping loading until the trailing request', async () => {
    const first = deferred<ClipboardItem[]>();
    const second = deferred<ClipboardItem[]>();
    const api = historyApi();
    api.listHistory
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const store = createHistoryStore(api);

    const refreshes = [
      store.refresh(),
      store.refresh(),
      store.refresh(),
      store.refresh()
    ];
    expect(api.listHistory).toHaveBeenCalledTimes(1);

    first.resolve([fixtures[0]]);
    await vi.waitFor(() => expect(api.listHistory).toHaveBeenCalledTimes(2));
    expect(get(store).items).toEqual([]);
    expect(get(store).loading).toBe(true);

    second.resolve(fixtures);
    await Promise.all(refreshes);

    expect(api.listHistory).toHaveBeenCalledTimes(2);
    expect(get(store).items).toEqual(fixtures);
    expect(get(store).loading).toBe(false);
  });

  it('establishes single-flight before publishing loading state to subscribers', async () => {
    const first = deferred<ClipboardItem[]>();
    const second = deferred<ClipboardItem[]>();
    const api = historyApi();
    let active = 0;
    let maxActive = 0;
    let call = 0;
    api.listHistory.mockImplementation(async () => {
      active += 1;
      maxActive = Math.max(maxActive, active);
      const response = call++ === 0 ? first : second;
      try {
        return await response.promise;
      } finally {
        active -= 1;
      }
    });
    const store = createHistoryStore(api);
    let reentrantRefresh: Promise<void> | null = null;
    const unsubscribe = store.subscribe((state) => {
      if (state.loading && reentrantRefresh === null) {
        reentrantRefresh = store.refresh();
      }
    });

    const initialRefresh = store.refresh();

    expect(api.listHistory).toHaveBeenCalledTimes(1);
    expect(maxActive).toBe(1);
    first.resolve(fixtures);
    await vi.waitFor(() => expect(api.listHistory).toHaveBeenCalledTimes(2));
    second.resolve(fixtures);
    await Promise.all([initialRefresh, reentrantRefresh]);
    unsubscribe();

    expect(api.listHistory).toHaveBeenCalledTimes(2);
    expect(maxActive).toBe(1);
  });

  it('ignores a superseded refresh error before the trailing refresh succeeds', async () => {
    const first = deferred<ClipboardItem[]>();
    const second = deferred<ClipboardItem[]>();
    const api = historyApi();
    api.listHistory
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const store = createHistoryStore(api);

    const imageQuery = store.setQuery({ kind: 'image', search: '' });
    const filesQuery = store.setQuery({ kind: 'files', search: '' });
    first.reject(new Error('stale failure'));
    await vi.waitFor(() => expect(api.listHistory).toHaveBeenCalledTimes(2));
    expect(get(store).error).toBeNull();
    expect(get(store).loading).toBe(true);

    second.resolve([fixtures[2]]);
    await Promise.all([imageQuery, filesQuery]);

    expect(get(store)).toMatchObject({
      items: [fixtures[2]],
      loading: false,
      error: null
    });
  });

  it('queries the backend again when a narrow filter expands to all', async () => {
    const api = historyApi();
    api.listHistory.mockImplementation(async (query) =>
      query.kind === 'image' ? [fixtures[1]] : fixtures
    );
    const store = createHistoryStore(api);

    await store.setQuery({ kind: 'image', search: '' });
    expect(get(store).visibleItems).toEqual([fixtures[1]]);

    await store.setQuery({ kind: 'all', search: '' });

    expect(api.listHistory.mock.calls).toEqual([
      [{ kind: 'image', search: '' }],
      [{ kind: null, search: '' }]
    ]);
    expect(get(store).visibleItems).toEqual(fixtures);
  });

  it('queries for a tail search match beyond an earlier result limit', async () => {
    const tailMatch: ClipboardItem = {
      ...fixtures[0],
      id: 'tail-121',
      preview: 'Hidden needle'
    };
    const api = historyApi();
    api.listHistory.mockImplementation(async (query) =>
      query.search === 'needle' ? [tailMatch] : fixtures
    );
    const store = createHistoryStore(api);

    await store.refresh();
    await store.setQuery({ kind: 'all', search: 'needle' });

    expect(api.listHistory.mock.calls).toEqual([
      [{ kind: null, search: '' }],
      [{ kind: null, search: 'needle' }]
    ]);
    expect(get(store).items).toEqual([tailMatch]);
    expect(get(store).visibleItems).toEqual([tailMatch]);
    expect(get(store).selectedId).toBe('tail-121');
  });

  it('copies query input before an asynchronous refresh', async () => {
    const pending = deferred<ClipboardItem[]>();
    const api = historyApi();
    api.listHistory.mockImplementationOnce(() => pending.promise);
    const store = createHistoryStore(api);
    const query: { kind: 'image' | 'all'; search: string } = {
      kind: 'image',
      search: ''
    };

    const queryRefresh = store.setQuery(query);
    query.kind = 'all';
    query.search = 'changed';
    pending.resolve([fixtures[1]]);
    await queryRefresh;

    expect(api.listHistory).toHaveBeenCalledWith({ kind: 'image', search: '' });
    expect(get(store).query).toEqual({ kind: 'image', search: '' });
    expect(get(store).visibleItems).toEqual([fixtures[1]]);
  });

  it('does not let a pending refresh revive a successfully deleted item', async () => {
    const pending = deferred<ClipboardItem[]>();
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory.mockImplementationOnce(() => pending.promise);

    const refresh = store.refresh();
    const deletion = store.deleteItem('text-1');
    await vi.waitFor(() =>
      expect(get(store).items.map((item) => item.id)).toEqual(['image-1', 'files-1'])
    );

    expect(api.listHistory).toHaveBeenCalledTimes(2);
    expect(get(store)).toMatchObject({
      items: [fixtures[1], fixtures[2]],
      visibleItems: [fixtures[1], fixtures[2]],
      selectedId: 'image-1',
      loading: true,
      error: null
    });

    pending.resolve(fixtures);
    await Promise.all([refresh, deletion]);
    expect(api.listHistory).toHaveBeenCalledTimes(3);

    expect(get(store)).toMatchObject({
      items: [fixtures[1], fixtures[2]],
      visibleItems: [fixtures[1], fixtures[2]],
      selectedId: 'image-1',
      loading: false,
      error: null
    });
  });

  it('does not let a pending refresh revive items cleared successfully', async () => {
    const pending = deferred<ClipboardItem[]>();
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory.mockImplementationOnce(() => pending.promise);

    const refresh = store.refresh();
    const clearing = store.clearHistory();
    await vi.waitFor(() =>
      expect(get(store).items.map((item) => item.id)).toEqual(['image-1'])
    );
    expect(api.listHistory).toHaveBeenCalledTimes(2);
    pending.resolve(fixtures);
    await Promise.all([refresh, clearing]);
    expect(api.listHistory).toHaveBeenCalledTimes(3);

    expect(get(store)).toMatchObject({
      items: [fixtures[1]],
      visibleItems: [fixtures[1]],
      selectedId: 'image-1',
      loading: false,
      error: null
    });
  });

  it('refreshes the current query after favorite mutation and recovers a tail match', async () => {
    const staleQuery = deferred<ClipboardItem[]>();
    const authoritative = deferred<ClipboardItem[]>();
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory
      .mockImplementationOnce(() => staleQuery.promise)
      .mockImplementationOnce(() => authoritative.promise);
    const knownMatch = {
      ...fixtures[0],
      preview: 'Known needle',
      isFavorite: true
    };
    const tailMatch = {
      ...fixtures[2],
      id: 'tail-121',
      preview: 'Tail needle'
    };

    const queryRefresh = store.setQuery({ kind: 'all', search: 'needle' });
    const favorite = store.setFavorite('text-1', true);
    staleQuery.resolve([
      { ...knownMatch, isFavorite: false },
      tailMatch
    ]);
    await vi.waitFor(() => expect(api.listHistory).toHaveBeenCalledTimes(3));
    authoritative.resolve([knownMatch, tailMatch]);
    await Promise.all([queryRefresh, favorite]);

    expect(api.listHistory).toHaveBeenLastCalledWith({
      kind: null,
      search: 'needle'
    });
    expect(get(store).items).toEqual([knownMatch, tailMatch]);
    expect(get(store)).toMatchObject({
      selectedId: 'text-1',
      loading: false,
      error: null
    });

    expect(get(store).items).toEqual([knownMatch, tailMatch]);
  });

  it('ignores a pending refresh error after a successful mutation', async () => {
    const pending = deferred<ClipboardItem[]>();
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory.mockImplementationOnce(() => pending.promise);

    const refresh = store.refresh();
    const deletion = store.deleteItem('text-1');
    await vi.waitFor(() =>
      expect(get(store).items.map((item) => item.id)).toEqual(['image-1', 'files-1'])
    );
    expect(api.listHistory).toHaveBeenCalledTimes(2);
    pending.reject(new Error('stale refresh failure'));
    await Promise.all([refresh, deletion]);
    expect(api.listHistory).toHaveBeenCalledTimes(3);

    expect(get(store)).toMatchObject({
      items: [fixtures[1], fixtures[2]],
      visibleItems: [fixtures[1], fixtures[2]],
      selectedId: 'image-1',
      loading: false,
      error: null
    });
  });

  it('allows a pending refresh to commit after a mutation fails', async () => {
    const pending = deferred<ClipboardItem[]>();
    const api = historyApi();
    api.deleteItem.mockRejectedValueOnce(new Error('delete failed'));
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory.mockImplementationOnce(() => pending.promise);

    const refresh = store.refresh();
    await store.deleteItem('text-1');
    expect(api.listHistory).toHaveBeenCalledTimes(2);
    expect(get(store)).toMatchObject({
      items: fixtures,
      loading: true,
      error: 'delete failed'
    });

    pending.resolve([fixtures[1], fixtures[2]]);
    await refresh;

    expect(get(store)).toMatchObject({
      items: [fixtures[1], fixtures[2]],
      visibleItems: [fixtures[1], fixtures[2]],
      selectedId: 'image-1',
      loading: false,
      error: 'delete failed'
    });
  });

  it('does not clear loading owned by a refresh started after a mutation', async () => {
    const oldRefresh = deferred<ClipboardItem[]>();
    const newRefresh = deferred<ClipboardItem[]>();
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory
      .mockImplementationOnce(() => oldRefresh.promise)
      .mockImplementationOnce(() => newRefresh.promise);
    const mutationApplied = new Promise<void>((resolve) => {
      let unsubscribe: () => void = () => undefined;
      unsubscribe = store.subscribe((state) => {
        if (!state.items.some((item) => item.id === 'text-1')) {
          unsubscribe();
          resolve();
        }
      });
    });

    const refreshA = store.refresh();
    const deletion = store.deleteItem('text-1');
    await mutationApplied;
    expect(api.listHistory).toHaveBeenCalledTimes(2);
    expect(get(store).loading).toBe(true);

    oldRefresh.resolve(fixtures);
    await vi.waitFor(() => expect(api.listHistory).toHaveBeenCalledTimes(3));

    expect(get(store).loading).toBe(true);
    expect(get(store).items.map((item) => item.id)).toEqual(['image-1', 'files-1']);

    newRefresh.resolve([fixtures[1], fixtures[2]]);
    await Promise.all([refreshA, deletion]);
    expect(get(store)).toMatchObject({
      items: [fixtures[1], fixtures[2]],
      visibleItems: [fixtures[1], fixtures[2]],
      selectedId: 'image-1',
      loading: false,
      error: null
    });
  });

  it('reorders successful favorite changes like the SQLite query', async () => {
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();

    await store.setFavorite('text-1', true);
    expect(get(store).items.map((item) => item.id)).toEqual([
      'image-1',
      'text-1',
      'files-1'
    ]);

    await store.setFavorite('image-1', false);
    expect(get(store).items.map((item) => item.id)).toEqual([
      'text-1',
      'files-1',
      'image-1'
    ]);
    expect(api.listHistory).toHaveBeenCalledTimes(3);
  });

  it('applies successful delete and clear commands without selection regressions', async () => {
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();

    await store.deleteItem('text-1');
    expect(get(store).items.map((item) => item.id)).toEqual(['image-1', 'files-1']);
    expect(get(store).selectedId).toBe('image-1');

    await store.clearHistory();
    expect(get(store).items.map((item) => item.id)).toEqual(['image-1']);
    expect(get(store).selectedId).toBe('image-1');
    expect(api.listHistory).toHaveBeenCalledTimes(3);
  });

  it('keeps a safe local mutation and exposes an authoritative refresh failure', async () => {
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    api.listHistory.mockRejectedValueOnce(new Error('refresh failed'));

    await store.deleteItem('text-1');

    expect(get(store)).toMatchObject({
      items: [fixtures[1], fixtures[2]],
      visibleItems: [fixtures[1], fixtures[2]],
      selectedId: 'image-1',
      loading: false,
      error: 'refresh failed'
    });
  });

  it('keeps selection and ordering unchanged after copy and paste commands', async () => {
    const api = historyApi();
    const store = createHistoryStore(api);
    await store.refresh();
    const before = get(store);

    await store.copyItem('files-1');
    const directPasteSucceeded = await store.pasteItem('image-1');
    const selectedPasteSucceeded = await store.pasteSelected();

    expect(api.copyItem).toHaveBeenCalledWith('files-1');
    expect(api.pasteItem.mock.calls).toEqual([['image-1'], ['text-1']]);
    expect(directPasteSucceeded).toBe(true);
    expect(selectedPasteSucceeded).toBe(true);
    expect(get(store)).toEqual(before);
  });

  it('returns false when paste fails while preserving the command error', async () => {
    const api = historyApi();
    api.pasteItem.mockRejectedValueOnce(new Error('paste failed'));
    const store = createHistoryStore(api);
    await store.refresh();

    const succeeded = await store.pasteSelected();

    expect(succeeded).toBe(false);
    expect(get(store).error).toBe('paste failed');
  });

  it('records command errors without applying optimistic state', async () => {
    const api = historyApi();
    api.deleteItem.mockRejectedValueOnce(new Error('delete failed'));
    const store = createHistoryStore(api);
    await store.refresh();

    await store.deleteItem('text-1');

    expect(get(store).items).toEqual(fixtures);
    expect(get(store).error).toBe('delete failed');
  });
});

const validSettings: AppSettings = {
  historyLimit: 100,
  favoriteLimit: 20,
  maxItemBytes: 50 * 1024 * 1024,
  hotkey: 'Ctrl+Shift+V',
  theme: 'system',
  motionScale: 1,
  autostart: false,
  clearOnExit: false
};

describe('settings state', () => {
  it('mirrors backend validation for item size and motion scale', () => {
    expect(validateSettings(validSettings)).toEqual({});
    expect(validateSettings({ ...validSettings, maxItemBytes: 0 })).toHaveProperty(
      'maxItemBytes'
    );
    expect(
      validateSettings({ ...validSettings, motionScale: Number.POSITIVE_INFINITY })
    ).toHaveProperty('motionScale');
    expect(
      validateSettings({ ...validSettings, motionScale: 2.01 })
    ).toHaveProperty('motionScale');
    expect(validateSettings({ ...validSettings, hotkey: '   ' })).toHaveProperty(
      'hotkey'
    );
    expect(
      validateSettings({
        ...validSettings,
        theme: 'unknown' as AppSettings['theme']
      })
    ).toHaveProperty('theme');
  });

  it('accepts valid remote settings and rejects malformed event payloads', async () => {
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi.fn(async (_settings: AppSettings) => undefined)
    };
    const store = createSettingsStore(api);
    await store.load();
    const remote = { ...validSettings, theme: 'dark' as const, motionScale: 0.7 };

    expect(store.receive(remote)).toBe(true);
    expect(get(store).value).toEqual(remote);
    expect(store.receive({ ...remote, hotkey: '' })).toBe(false);
    expect(() => store.receive({})).not.toThrow();
    expect(store.receive({})).toBe(false);
    expect(store.receive(null)).toBe(false);
    expect(get(store).value).toEqual(remote);
  });

  it('loads and saves settings while exposing validation errors', async () => {
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi.fn(async (_settings: AppSettings) => undefined)
    };
    const store = createSettingsStore(api);

    await store.load();
    expect(get(store).value).toEqual(validSettings);

    await store.save({ ...validSettings, maxItemBytes: 0 });
    expect(api.saveSettings).not.toHaveBeenCalled();
    expect(get(store).validationErrors).toHaveProperty('maxItemBytes');

    const changed = { ...validSettings, theme: 'dark' as const };
    await store.save(changed);
    expect(api.saveSettings).toHaveBeenCalledWith(changed);
    expect(get(store)).toMatchObject({
      value: changed,
      saving: false,
      error: null,
      validationErrors: {}
    });
  });

  it('does not let an old load overwrite a newer successful save', async () => {
    const loadResult = deferred<AppSettings>();
    const saveResult = deferred<void>();
    const api = {
      getSettings: vi.fn(() => loadResult.promise),
      saveSettings: vi.fn(() => saveResult.promise)
    };
    const store = createSettingsStore(api);
    const changed = { ...validSettings, theme: 'dark' as const };

    const load = store.load();
    const save = store.save(changed);
    saveResult.resolve();
    await save;

    expect(get(store)).toMatchObject({
      value: changed,
      loading: true,
      saving: false,
      error: null
    });

    loadResult.resolve(validSettings);
    await load;
    expect(get(store)).toMatchObject({
      value: changed,
      loading: false,
      saving: false,
      error: null
    });
  });

  it('keeps the latest load result and accurate loading state', async () => {
    const first = deferred<AppSettings>();
    const second = deferred<AppSettings>();
    const api = {
      getSettings: vi
        .fn()
        .mockImplementationOnce(() => first.promise)
        .mockImplementationOnce(() => second.promise),
      saveSettings: vi.fn(async (_settings: AppSettings) => undefined)
    };
    const store = createSettingsStore(api);
    const latest = { ...validSettings, theme: 'dark' as const };

    const loadA = store.load();
    const loadB = store.load();
    second.resolve(latest);
    await loadB;

    expect(get(store)).toMatchObject({ value: latest, loading: true, error: null });

    first.resolve(validSettings);
    await loadA;
    expect(get(store)).toMatchObject({ value: latest, loading: false, error: null });
  });

  it('serializes saves so the final backend and state value are the last call', async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi
        .fn()
        .mockImplementationOnce(() => first.promise)
        .mockImplementationOnce(() => second.promise)
    };
    const store = createSettingsStore(api);
    const firstSettings = { ...validSettings, theme: 'light' as const };
    const lastSettings = { ...validSettings, theme: 'dark' as const };

    const saveA = store.save(firstSettings);
    const saveB = store.save(lastSettings);
    await Promise.resolve();

    expect(api.saveSettings.mock.calls).toEqual([[firstSettings]]);
    expect(get(store).saving).toBe(true);

    first.resolve();
    await saveA;
    expect(api.saveSettings.mock.calls).toEqual([[firstSettings], [lastSettings]]);
    expect(get(store).saving).toBe(true);

    second.resolve();
    await saveB;
    expect(get(store)).toMatchObject({
      value: lastSettings,
      saving: false,
      error: null
    });
  });

  it('keeps the last successful backend value when a newer save fails', async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi
        .fn()
        .mockImplementationOnce(() => first.promise)
        .mockImplementationOnce(() => second.promise)
    };
    const store = createSettingsStore(api);
    const persisted = { ...validSettings, theme: 'light' as const };
    const rejected = { ...validSettings, theme: 'dark' as const };

    const saveA = store.save(persisted);
    const saveB = store.save(rejected);
    first.resolve();
    await saveA;

    expect(get(store)).toMatchObject({
      value: persisted,
      saving: true,
      error: null
    });

    second.reject(new Error('second save failed'));
    await expect(saveB).rejects.toThrow('second save failed');
    expect(get(store)).toMatchObject({
      value: persisted,
      saving: false,
      error: 'second save failed'
    });
  });

  it('advances persisted value without clearing a newer invalid-save error', async () => {
    const pending = deferred<void>();
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi.fn(() => pending.promise)
    };
    const store = createSettingsStore(api);
    const persisted = { ...validSettings, theme: 'dark' as const };

    const validSave = store.save(persisted);
    await Promise.resolve();
    await store.save({ ...validSettings, maxItemBytes: 0 });

    expect(get(store).validationErrors).toHaveProperty('maxItemBytes');
    pending.resolve();
    await validSave;

    expect(get(store)).toMatchObject({
      value: persisted,
      saving: false,
      error: null
    });
    expect(get(store).validationErrors).toHaveProperty('maxItemBytes');
  });

  it('does not expose an old load failure after a newer save succeeds', async () => {
    const loadResult = deferred<AppSettings>();
    const api = {
      getSettings: vi.fn(() => loadResult.promise),
      saveSettings: vi.fn(async (_settings: AppSettings) => undefined)
    };
    const store = createSettingsStore(api);
    const changed = { ...validSettings, theme: 'dark' as const };

    const load = store.load();
    await store.save(changed);
    loadResult.reject(new Error('stale load failed'));
    await expect(load).rejects.toThrow('stale load failed');

    expect(get(store)).toMatchObject({
      value: changed,
      loading: false,
      saving: false,
      error: null
    });
  });

  it('snapshots save input before queuing and before storing it', async () => {
    const saveResult = deferred<void>();
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi.fn(() => saveResult.promise)
    };
    const store = createSettingsStore(api);
    const requested: AppSettings = { ...validSettings, theme: 'dark' };

    const save = store.save(requested);
    requested.theme = 'light';
    requested.hotkey = 'mutated';
    await Promise.resolve();

    expect(api.saveSettings).toHaveBeenCalledWith({
      ...validSettings,
      theme: 'dark'
    });

    saveResult.resolve();
    await save;
    expect(get(store).value).toEqual({ ...validSettings, theme: 'dark' });
  });

  it('keeps invalid-save validation when an older load finishes', async () => {
    const loadResult = deferred<AppSettings>();
    const api = {
      getSettings: vi.fn(() => loadResult.promise),
      saveSettings: vi.fn(async (_settings: AppSettings) => undefined)
    };
    const store = createSettingsStore(api);

    const load = store.load();
    await store.save({ ...validSettings, maxItemBytes: 0 });
    expect(get(store)).toMatchObject({
      value: null,
      loading: true,
      saving: false
    });
    expect(get(store).validationErrors).toHaveProperty('maxItemBytes');

    loadResult.resolve(validSettings);
    await load;
    expect(get(store)).toMatchObject({
      value: null,
      loading: false,
      saving: false,
      error: null
    });
    expect(get(store).validationErrors).toHaveProperty('maxItemBytes');
  });

  it('records and rethrows the latest backend failure', async () => {
    const api = {
      getSettings: vi.fn(async () => validSettings),
      saveSettings: vi.fn(async (_settings: AppSettings) => {
        throw new Error('save failed');
      })
    };
    const store = createSettingsStore(api);

    await expect(store.save(validSettings)).rejects.toThrow('save failed');
    expect(get(store)).toMatchObject({
      value: null,
      saving: false,
      error: 'save failed'
    });
  });
});
