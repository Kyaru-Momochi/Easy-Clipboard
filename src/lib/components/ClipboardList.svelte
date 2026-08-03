<script lang="ts">
  import type { ClipboardItem, ItemId } from '../types';
  import ClipboardCard from './ClipboardCard.svelte';
  import type { MenuPosition } from './context-menu';

  interface Props {
    items: ClipboardItem[];
    selectedId: ItemId | null;
    busyId: ItemId | null;
    loading: boolean;
    error: string | null;
    onPaste: (item: ClipboardItem) => void | Promise<void>;
    onSelect: (item: ClipboardItem) => void;
    onContextMenu: (item: ClipboardItem, position: MenuPosition) => void;
  }

  let {
    items,
    selectedId,
    busyId,
    loading,
    error,
    onPaste,
    onSelect,
    onContextMenu
  }: Props = $props();

  let renderedItems = $derived.by(() => {
    let favoriteCount = 0;
    let normalCount = 0;
    return items.filter((item) => {
      if (item.isFavorite) {
        favoriteCount += 1;
        return favoriteCount <= 20;
      }
      normalCount += 1;
      return normalCount <= 100;
    });
  });
</script>

<section class="clipboard-list" aria-label="剪贴板历史">
  {#if error}
    <p class="state error" role="alert">{error}</p>
  {/if}

  {#if loading && items.length === 0}
    <p class="state" role="status">正在加载剪贴板历史…</p>
  {:else if items.length === 0 && !error}
    <p class="state" role="status">暂无剪贴板历史</p>
  {:else}
    {#if loading}
      <p class="syncing" role="status">正在同步…</p>
    {/if}
    <div class="cards">
      {#each renderedItems as item (item.id)}
        <ClipboardCard
          {item}
          selected={selectedId === item.id}
          busy={busyId === item.id}
          {onPaste}
          {onSelect}
          {onContextMenu}
        />
      {/each}
    </div>
    <p class="count">显示 {renderedItems.length} / {items.length} 条</p>
  {/if}
</section>

<style>
  .clipboard-list {
    position: relative;
    min-height: 0;
  }

  .cards {
    display: grid;
    gap: 0.55rem;
  }

  .state {
    display: grid;
    min-height: 8rem;
    margin: 0;
    place-items: center;
    color: var(--ui-muted, #697181);
    text-align: center;
  }

  .state.error {
    min-height: auto;
    margin-bottom: 0.55rem;
    padding: 0.65rem 0.75rem;
    border-radius: 0.6rem;
    color: var(--ui-danger, #a23838);
    background: #fff0f0;
  }

  .syncing,
  .count {
    margin: 0.55rem 0 0;
    color: var(--ui-muted, #697181);
    font-size: 0.75rem;
    text-align: right;
  }
</style>
