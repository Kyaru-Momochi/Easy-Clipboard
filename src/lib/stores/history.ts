import { writable } from 'svelte/store';

import * as api from '../api';
import type {
  ClipboardItem,
  HistoryFilter,
  HistoryQuery,
  ItemId
} from '../types';

export interface HistoryState {
  /** Latest backend result for `query`; this is not an unfiltered global cache. */
  items: ClipboardItem[];
  visibleItems: ClipboardItem[];
  query: HistoryFilter;
  selectedId: ItemId | null;
  loading: boolean;
  error: string | null;
}

export interface HistoryApi {
  listHistory(query: HistoryQuery): Promise<ClipboardItem[]>;
  pasteItem(id: ItemId): Promise<void>;
  copyItem(id: ItemId): Promise<void>;
  setFavorite(id: ItemId, value: boolean): Promise<void>;
  deleteItem(id: ItemId): Promise<void>;
  clearHistory(): Promise<void>;
}

const DEFAULT_QUERY: Readonly<HistoryFilter> = Object.freeze({
  kind: 'all',
  search: ''
});

export function filterItems(
  items: readonly ClipboardItem[],
  query: HistoryFilter
): ClipboardItem[] {
  const search = query.search.toLowerCase();
  return items.filter(
    (item) =>
      (query.kind === 'all' || item.kind === query.kind) &&
      (search === '' || item.preview.toLowerCase().includes(search))
  );
}

export function moveSelection(
  ids: readonly ItemId[],
  currentId: ItemId | null,
  direction: -1 | 1
): ItemId | null {
  if (ids.length === 0) {
    return null;
  }

  const currentIndex = currentId === null ? -1 : ids.indexOf(currentId);
  if (currentIndex === -1) {
    return direction > 0 ? ids[0] : ids[ids.length - 1];
  }

  const nextIndex = Math.max(0, Math.min(ids.length - 1, currentIndex + direction));
  return ids[nextIndex];
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

function reconcile(state: HistoryState, items = state.items): HistoryState {
  const visibleItems = filterItems(items, state.query);
  const visibleIds = visibleItems.map((item) => item.id);
  const selectedId = visibleIds.includes(state.selectedId ?? '')
    ? state.selectedId
    : (visibleIds[0] ?? null);
  return { ...state, items, visibleItems, selectedId };
}

function compareHistoryItems(left: ClipboardItem, right: ClipboardItem): number {
  if (left.isFavorite !== right.isFavorite) {
    return left.isFavorite ? -1 : 1;
  }
  if (left.updatedAtMs !== right.updatedAtMs) {
    return left.updatedAtMs > right.updatedAtMs ? -1 : 1;
  }
  if (left.id === right.id) {
    return 0;
  }
  return left.id < right.id ? -1 : 1;
}

export function createHistoryStore(backend: HistoryApi = api) {
  const initialState: HistoryState = {
    items: [],
    visibleItems: [],
    query: { ...DEFAULT_QUERY },
    selectedId: null,
    loading: false,
    error: null
  };
  const { subscribe, update } = writable(initialState);
  let currentQuery: HistoryFilter = { ...DEFAULT_QUERY };
  let refreshEpoch = 0;
  let mutationRevision = 0;

  async function refresh(): Promise<void> {
    const epoch = ++refreshEpoch;
    const revision = mutationRevision;
    const query = { ...currentQuery };
    update((state) => ({ ...state, loading: true, error: null }));
    try {
      const items = await backend.listHistory({
        kind: query.kind === 'all' ? null : query.kind,
        search: query.search
      });
      if (epoch !== refreshEpoch) {
        return;
      }
      if (revision !== mutationRevision) {
        update((state) => ({ ...state, loading: false }));
        return;
      }
      update((state) => ({ ...reconcile(state, [...items]), loading: false }));
    } catch (error) {
      if (epoch !== refreshEpoch) {
        return;
      }
      if (revision !== mutationRevision) {
        update((state) => ({ ...state, loading: false }));
        return;
      }
      update((state) => ({ ...state, loading: false, error: errorMessage(error) }));
    }
  }

  function setQuery(query: HistoryFilter): Promise<void> {
    currentQuery = { ...query };
    update((state) => reconcile({ ...state, query: { ...currentQuery } }));
    return refresh();
  }

  function select(selectedId: ItemId | null): void {
    update((state) => ({
      ...state,
      selectedId: state.visibleItems.some((item) => item.id === selectedId)
        ? selectedId
        : null
    }));
  }

  function move(direction: -1 | 1): void {
    update((state) => ({
      ...state,
      selectedId: moveSelection(
        state.visibleItems.map((item) => item.id),
        state.selectedId,
        direction
      )
    }));
  }

  async function runCommand(command: () => Promise<void>): Promise<boolean> {
    update((state) => ({ ...state, error: null }));
    try {
      await command();
      return true;
    } catch (error) {
      update((state) => ({ ...state, error: errorMessage(error) }));
      return false;
    }
  }

  function commitItemsMutation(change: (items: ClipboardItem[]) => ClipboardItem[]): void {
    mutationRevision += 1;
    update((state) =>
      reconcile(
        {
          ...state,
          loading: false,
          error: null
        },
        change(state.items)
      )
    );
  }

  async function pasteItem(id: ItemId): Promise<void> {
    await runCommand(() => backend.pasteItem(id));
  }

  async function pasteSelected(): Promise<void> {
    let selectedId: ItemId | null = null;
    update((state) => {
      selectedId = state.selectedId;
      return state;
    });
    if (selectedId !== null) {
      await pasteItem(selectedId);
    }
  }

  async function copyItem(id: ItemId): Promise<void> {
    await runCommand(() => backend.copyItem(id));
  }

  async function setFavorite(id: ItemId, value: boolean): Promise<void> {
    if (await runCommand(() => backend.setFavorite(id, value))) {
      commitItemsMutation((items) =>
        items
          .map((item) => (item.id === id ? { ...item, isFavorite: value } : item))
          .sort(compareHistoryItems)
      );
      await refresh();
    }
  }

  async function deleteItem(id: ItemId): Promise<void> {
    if (await runCommand(() => backend.deleteItem(id))) {
      commitItemsMutation((items) => items.filter((item) => item.id !== id));
      await refresh();
    }
  }

  async function clearHistory(): Promise<void> {
    if (await runCommand(() => backend.clearHistory())) {
      commitItemsMutation((items) => items.filter((item) => item.isFavorite));
      await refresh();
    }
  }

  return {
    subscribe,
    refresh,
    setQuery,
    select,
    move,
    pasteItem,
    pasteSelected,
    copyItem,
    setFavorite,
    deleteItem,
    clearHistory
  };
}

export const historyStore = createHistoryStore();
