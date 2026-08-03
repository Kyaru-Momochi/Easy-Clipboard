<script lang="ts">
  import { onMount, tick } from 'svelte';

  import type { ClipboardItem } from '../types';
  import { clampMenuPosition, type MenuPosition } from './context-menu';

  interface Props {
    item: ClipboardItem;
    position: MenuPosition;
    busy: boolean;
    onClose: () => void;
    onCopy: (item: ClipboardItem) => void | Promise<void>;
    onFavorite: (item: ClipboardItem) => void | Promise<void>;
    onDelete: (item: ClipboardItem) => void | Promise<void>;
    onReveal: (item: ClipboardItem) => void | Promise<void>;
  }

  let {
    item,
    position,
    busy,
    onClose,
    onCopy,
    onFavorite,
    onDelete,
    onReveal
  }: Props = $props();
  let menuElement: HTMLDivElement;
  let menuWidth = $state(224);
  let menuHeight = $state(176);
  let viewportWidth = $state(typeof window === 'undefined' ? 1024 : window.innerWidth);
  let viewportHeight = $state(typeof window === 'undefined' ? 768 : window.innerHeight);

  let placed = $derived(
    clampMenuPosition(
      position.x,
      position.y,
      menuWidth,
      menuHeight,
      viewportWidth,
      viewportHeight
    )
  );
  let canReveal = $derived(
    item.payload.type === 'files' && item.payload.entries.length > 0
  );

  onMount(async () => {
    await tick();
    const bounds = menuElement.getBoundingClientRect();
    menuWidth = bounds.width || menuWidth;
    menuHeight = bounds.height || menuHeight;
    menuElement.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
  });

  function resize(): void {
    viewportWidth = window.innerWidth;
    viewportHeight = window.innerHeight;
  }

  function outside(event: PointerEvent): void {
    if (!menuElement.contains(event.target as Node)) {
      onClose();
    }
  }

  function keydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
    }
  }

  async function run(
    event: MouseEvent,
    action: (item: ClipboardItem) => void | Promise<void>
  ): Promise<void> {
    event.stopPropagation();
    if (busy) {
      return;
    }
    await action(item);
    onClose();
  }
</script>

<svelte:window onpointerdown={outside} onkeydown={keydown} onresize={resize} />

<div
  bind:this={menuElement}
  class="context-menu"
  role="menu"
  tabindex="-1"
  aria-label="剪贴记录操作"
  style:left={`${placed.x}px`}
  style:top={`${placed.y}px`}
  oncontextmenu={(event) => event.preventDefault()}
>
  <button type="button" role="menuitem" disabled={busy} onclick={(event) => run(event, onCopy)}>
    仅复制
  </button>
  <button
    type="button"
    role="menuitem"
    disabled={busy}
    onclick={(event) => run(event, onFavorite)}
  >
    {item.isFavorite ? '取消收藏' : '收藏'}
  </button>
  <button type="button" role="menuitem" disabled={busy} onclick={(event) => run(event, onDelete)}>
    删除
  </button>
  <button
    type="button"
    role="menuitem"
    disabled={busy || !canReveal}
    onclick={(event) => run(event, onReveal)}
  >
    打开文件位置
  </button>
</div>

<style>
  .context-menu {
    position: fixed;
    z-index: 100;
    display: grid;
    width: min(14rem, calc(100vw - 1rem));
    padding: 0.35rem;
    border: 1px solid var(--ui-border, #d7dce5);
    border-radius: 0.7rem;
    background: var(--ui-card, #fff);
    box-shadow: 0 0.75rem 2rem rgb(25 32 50 / 18%);
  }

  button {
    min-height: 2.35rem;
    padding: 0 0.7rem;
    border: 0;
    border-radius: 0.45rem;
    color: inherit;
    text-align: left;
    background: transparent;
    font: inherit;
    cursor: pointer;
  }

  button:hover:not(:disabled),
  button:focus-visible {
    outline: 0;
    background: var(--ui-control, #eef1f6);
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }
</style>
