<script lang="ts">
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';

  import { hideOverlay, openSettings, revealFile } from '../api';
  import { historyStore } from '../stores/history';
  import type { ClipboardItem, HistoryFilter } from '../types';
  import ClipboardList from './ClipboardList.svelte';
  import ContextMenu from './ContextMenu.svelte';
  import FilterBar from './FilterBar.svelte';
  import SearchField from './SearchField.svelte';
  import type { MenuPosition } from './context-menu';

  interface NativeEvent {
    payload: unknown;
  }

  type HistoryStore = typeof historyStore;
  type ListenToEvent = (
    event: string,
    handler: (event: NativeEvent) => void
  ) => Promise<UnlistenFn>;

  interface Props {
    store?: HistoryStore;
    listenToEvent?: ListenToEvent;
    openSettingsAction?: () => Promise<void>;
    revealFileAction?: (path: string) => Promise<void>;
    hideOverlayAction?: () => Promise<void>;
  }

  const defaultListen: ListenToEvent = (event, handler) =>
    listen<unknown>(event, (nativeEvent) => handler({ payload: nativeEvent.payload }));

  let {
    store = historyStore,
    listenToEvent = defaultListen,
    openSettingsAction = openSettings,
    revealFileAction = revealFile,
    hideOverlayAction = hideOverlay
  }: Props = $props();

  let contextMenu = $state<{ item: ClipboardItem; position: MenuPosition } | null>(null);
  let busyId = $state<string | null>(null);
  let settingsBusy = $state(false);
  let listenerError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let pasteFeedback = $state<string | null>(null);
  let displayedError = $derived(listenerError ?? actionError ?? $store.error);

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

  function setSearch(search: string): void {
    void store.setQuery({ ...$store.query, search });
  }

  function setFilter(kind: HistoryFilter['kind']): void {
    void store.setQuery({ ...$store.query, kind });
  }

  async function runItemAction(id: string, action: () => Promise<void>): Promise<void> {
    if (busyId !== null) {
      return;
    }
    busyId = id;
    actionError = null;
    pasteFeedback = null;
    try {
      await action();
    } catch (error) {
      actionError = errorMessage(error);
    } finally {
      busyId = null;
    }
  }

  function select(item: ClipboardItem): void {
    store.select(item.id);
  }

  function showContextMenu(item: ClipboardItem, position: MenuPosition): void {
    store.select(item.id);
    contextMenu = { item, position };
  }

  function paste(item: ClipboardItem): Promise<void> {
    return runItemAction(item.id, async () => {
      if (await store.pasteItem(item.id)) {
        await finishSuccessfulPaste();
      }
    });
  }

  function copy(item: ClipboardItem): Promise<void> {
    return runItemAction(item.id, () => store.copyItem(item.id));
  }

  function favorite(item: ClipboardItem): Promise<void> {
    return runItemAction(item.id, () => store.setFavorite(item.id, !item.isFavorite));
  }

  function remove(item: ClipboardItem): Promise<void> {
    return runItemAction(item.id, () => store.deleteItem(item.id));
  }

  function reveal(item: ClipboardItem): Promise<void> {
    return runItemAction(item.id, async () => {
      if (item.payload.type === 'files' && item.payload.entries.length > 0) {
        await revealFileAction(item.payload.entries[0].path);
      }
    });
  }

  async function showSettings(): Promise<void> {
    if (settingsBusy) {
      return;
    }
    settingsBusy = true;
    actionError = null;
    try {
      await openSettingsAction();
    } catch (error) {
      actionError = errorMessage(error);
    } finally {
      settingsBusy = false;
    }
  }

  function handleSearchEvent(event: NativeEvent): void {
    const payload = event.payload;
    if (typeof payload !== 'object' || payload === null || !('action' in payload)) {
      return;
    }
    if (
      payload.action === 'insert' &&
      'text' in payload &&
      typeof payload.text === 'string'
    ) {
      setSearch($store.query.search + payload.text);
    } else if (payload.action === 'deleteBackward') {
      setSearch(Array.from($store.query.search).slice(0, -1).join(''));
    }
  }

  function handleMoveEvent(event: NativeEvent): void {
    const payload = event.payload;
    if (
      typeof payload === 'object' &&
      payload !== null &&
      'delta' in payload &&
      typeof payload.delta === 'number' &&
      payload.delta !== 0
    ) {
      store.move(payload.delta < 0 ? -1 : 1);
    }
  }

  function handlePasteEvent(): void {
    if (contextMenu !== null) {
      return;
    }
    const selected = $store.visibleItems.find((item) => item.id === $store.selectedId);
    const unavailable =
      selected?.payload.type === 'files' &&
      selected.payload.entries.some((entry) => !entry.available);
    if (!selected || unavailable) {
      return;
    }
    void runItemAction(selected.id, async () => {
      if (await store.pasteSelected()) {
        await finishSuccessfulPaste();
      }
    });
  }

  async function finishSuccessfulPaste(): Promise<void> {
    pasteFeedback = '已粘贴';
    contextMenu = null;
    store.select(null);
    try {
      await hideOverlayAction();
    } catch (error) {
      actionError = errorMessage(error);
    }
  }

  async function handleHideEvent(): Promise<void> {
    contextMenu = null;
    actionError = null;
    pasteFeedback = null;
    store.select(null);
    try {
      await hideOverlayAction();
    } catch (error) {
      actionError = errorMessage(error);
    }
  }

  function handleHistoryChanged(): void {
    pasteFeedback = null;
    void store.refresh();
  }

  function consumeDismissPointer(event: PointerEvent): void {
    event.preventDefault();
    event.stopPropagation();
  }

  function dismissContextMenu(event: MouseEvent): void {
    event.preventDefault();
    event.stopPropagation();
    contextMenu = null;
  }

  onMount(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const registrations = [
      ['search-input', handleSearchEvent],
      ['selection-move', handleMoveEvent],
      ['selection-paste', handlePasteEvent],
      ['overlay-hide', () => void handleHideEvent()],
      ['history-changed', handleHistoryChanged]
    ] as const;

    void store.refresh();
    for (const [event, handler] of registrations) {
      void listenToEvent(event, handler)
        .then((unlisten) => {
          if (disposed) {
            unlisten();
          } else {
            unlisteners.push(unlisten);
          }
        })
        .catch((error) => {
          if (!disposed) {
            listenerError = `键盘事件监听失败：${errorMessage(error)}`;
          }
        });
    }

    return () => {
      disposed = true;
      for (const unlisten of unlisteners.splice(0)) {
        unlisten();
      }
    };
  });
</script>

<main
  class="app-shell"
  style="box-sizing: border-box; width: 100%; max-width: 720px;"
>
  <header>
    <div>
      <p class="eyebrow">Easy Clipboard</p>
      <h1>剪贴板历史</h1>
    </div>
    <span class="status" aria-live="polite">
      {$store.loading
        ? '同步中'
        : (pasteFeedback ?? `${$store.visibleItems.length} 条可见`)}
    </span>
  </header>

  <SearchField value={$store.query.search} onSearch={setSearch} />
  <FilterBar value={$store.query.kind} onChange={setFilter} />

  <div class="history-region">
    <ClipboardList
      items={$store.visibleItems}
      selectedId={$store.selectedId}
      {busyId}
      loading={$store.loading}
      error={displayedError}
      onPaste={paste}
      onSelect={select}
      onContextMenu={showContextMenu}
    />
  </div>

  <footer>
    <span>历史数量：{$store.items.length}</span>
    <button type="button" aria-label="打开设置" disabled={settingsBusy} onclick={showSettings}>
      设置
    </button>
  </footer>
</main>

{#if contextMenu}
  <button
    class="dismiss-layer"
    type="button"
    tabindex="-1"
    aria-label="关闭剪贴记录菜单"
    onpointerdown={consumeDismissPointer}
    onclick={dismissContextMenu}
  ></button>
  <ContextMenu
    item={contextMenu.item}
    position={contextMenu.position}
    busy={busyId === contextMenu.item.id}
    onClose={() => (contextMenu = null)}
    onCopy={copy}
    onFavorite={favorite}
    onDelete={remove}
    onReveal={reveal}
  />
{/if}

<style>
  .app-shell {
    --ui-accent: #5068d8;
    --ui-border: #d9dee7;
    --ui-card: #fff;
    --ui-control: #f1f3f7;
    --ui-muted: #687181;
    --ui-danger: #a23838;

    display: grid;
    grid-template-rows: auto auto auto minmax(0, 1fr) auto;
    gap: 0.75rem;
    height: 100dvh;
    min-height: 420px;
    max-height: 820px;
    margin: 0 auto;
    padding: 1rem;
    overflow: hidden;
    color: #202532;
    background: #fafbfc;
    font-family: Inter, 'Segoe UI', 'Microsoft YaHei UI', system-ui, sans-serif;
  }

  header,
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }

  h1,
  .eyebrow {
    margin: 0;
  }

  h1 {
    font-size: 1.15rem;
  }

  .eyebrow {
    margin-bottom: 0.15rem;
    color: var(--ui-muted);
    font-size: 0.72rem;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .status,
  footer {
    color: var(--ui-muted);
    font-size: 0.78rem;
  }

  .status {
    white-space: nowrap;
  }

  .history-region {
    min-height: 0;
    overflow: auto;
    scrollbar-gutter: stable;
  }

  footer button {
    min-height: 2rem;
    padding: 0 0.7rem;
    border: 1px solid var(--ui-border);
    border-radius: 0.55rem;
    color: inherit;
    background: var(--ui-card);
    font: inherit;
    cursor: pointer;
  }

  footer button:hover:not(:disabled),
  footer button:focus-visible {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    outline: 0;
  }

  .dismiss-layer {
    position: fixed;
    z-index: 90;
    inset: 0;
    width: 100vw;
    height: 100vh;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: default;
  }

  @media (max-width: 390px), (max-height: 520px) {
    .app-shell {
      gap: 0.55rem;
      padding: 0.7rem;
    }

    .eyebrow {
      display: none;
    }
  }
</style>
