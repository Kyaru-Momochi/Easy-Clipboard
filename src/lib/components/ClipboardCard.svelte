<script lang="ts">
  import { assetUrl } from '../api';
  import { formatBytes } from '../format';
  import type { ClipboardItem, FileEntry } from '../types';
  import type { MenuPosition } from './context-menu';

  interface Props {
    item: ClipboardItem;
    selected: boolean;
    busy: boolean;
    onPaste: (item: ClipboardItem) => void | Promise<void>;
    onSelect?: (item: ClipboardItem) => void;
    onContextMenu: (item: ClipboardItem, position: MenuPosition) => void;
  }

  let {
    item,
    selected,
    busy,
    onPaste,
    onSelect = () => undefined,
    onContextMenu
  }: Props = $props();
  let failedThumbnailPath = $state<string | null>(null);

  const MAX_VISIBLE_FILE_ENTRIES = 4;
  const mediaLabels: Record<FileEntry['mediaKind'], string> = {
    audio: '音频',
    video: '视频',
    other: '文件'
  };

  let unavailable = $derived(
    item.payload.type === 'files' &&
      item.payload.entries.some((entry) => !entry.available)
  );
  let availabilityPending = $derived(
    item.payload.type === 'files' &&
      item.payload.entries.some((entry) => entry.availabilityPending === true)
  );
  let thumbnailPath = $derived(
    item.payload.type === 'image' ? item.payload.thumbnailPath : ''
  );
  let thumbnailUrl = $derived(assetUrl(thumbnailPath));
  let visibleFileEntries = $derived(
    item.payload.type === 'files'
      ? item.payload.entries.slice(0, MAX_VISIBLE_FILE_ENTRIES)
      : []
  );
  let hiddenFileEntryCount = $derived(
    item.payload.type === 'files'
      ? Math.max(0, item.payload.entries.length - MAX_VISIBLE_FILE_ENTRIES)
      : 0
  );

  async function paste(): Promise<void> {
    if (unavailable || busy) {
      return;
    }
    onSelect(item);
    await onPaste(item);
  }

  function openContextMenu(event: MouseEvent): void {
    event.preventDefault();
    event.stopPropagation();
    onContextMenu(item, { x: event.clientX, y: event.clientY });
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="card-shell" oncontextmenu={openContextMenu}>
  <!-- Product contract requires native button semantics plus keyboard-selection state. -->
  <!-- svelte-ignore a11y_role_supports_aria_props_implicit -->
  <button
    class:selected
    type="button"
    aria-selected={selected}
    aria-busy={busy}
    disabled={unavailable || busy}
    onclick={paste}
  >
    {#if item.payload.type === 'text'}
      <span class="kind" aria-hidden="true">文本</span>
      <span class="summary">{item.payload.plain || item.preview}</span>
      <span class="meta">{formatBytes(item.byteSize)}</span>
    {:else if item.payload.type === 'image'}
      <span class="thumbnail">
        {#if thumbnailUrl !== '' && failedThumbnailPath !== thumbnailPath}
          <img
            src={thumbnailUrl}
            alt={`图片剪贴记录：${item.preview || `${item.payload.width} × ${item.payload.height}`}`}
            loading="lazy"
            onerror={() => (failedThumbnailPath = thumbnailPath)}
          />
        {:else}
          <span class="image-fallback">缩略图不可用</span>
        {/if}
      </span>
      <span class="card-body">
        <span class="summary">{item.preview || '图片'}</span>
        <span class="meta">
          <span>{item.payload.width} × {item.payload.height}</span>
          <span aria-hidden="true"> · </span>
          <span>{formatBytes(item.byteSize)}</span>
        </span>
      </span>
    {:else}
      <span class="file-stack">
        {#each visibleFileEntries as entry}
          <span class="file-entry">
            <span class="file-heading">
              <span class="kind">{mediaLabels[entry.mediaKind]}</span>
              <span class="summary">{entry.name}</span>
              <span class="meta">{formatBytes(entry.sizeBytes)}</span>
            </span>
            <span class="path">{entry.path}</span>
          </span>
        {/each}
        {#if hiddenFileEntryCount > 0}
          <span class="more-files">另有 {hiddenFileEntryCount} 项</span>
        {/if}
        {#if unavailable}
          <span class="unavailable">原文件不可用</span>
        {:else if availabilityPending}
          <span class="pending">部分文件待校验</span>
        {/if}
      </span>
    {/if}
    {#if item.isFavorite}
      <span class="favorite" aria-label="已收藏">★</span>
    {/if}
  </button>
</div>

<style>
  .card-shell {
    min-width: 0;
  }

  button {
    position: relative;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 0.7rem;
    align-items: center;
    width: 100%;
    min-height: 4.5rem;
    padding: 0.75rem;
    overflow: hidden;
    border: 1px solid var(--ui-border, #dfe3ea);
    border-radius: 0.8rem;
    color: inherit;
    text-align: left;
    background: var(--ui-card, #fff);
    font: inherit;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    border-color: var(--ui-accent, #5068d8);
    background: var(--ui-control, #f7f8fc);
  }

  button.selected {
    border-color: var(--ui-accent, #5068d8);
    box-shadow: inset 3px 0 var(--ui-accent, #5068d8);
  }

  button:focus-visible {
    outline: 2px solid var(--ui-accent, #5068d8);
    outline-offset: 2px;
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.68;
  }

  .kind {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 2.5rem;
    min-height: 1.55rem;
    padding: 0 0.35rem;
    border-radius: 0.45rem;
    color: var(--ui-accent, #4057bd);
    background: var(--ui-control, #eef1fb);
    font-size: 0.75rem;
    font-weight: 650;
  }

  .summary,
  .path,
  .meta {
    min-width: 0;
  }

  .summary {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }

  .meta,
  .path,
  .more-files {
    color: var(--ui-muted, #697181);
    font-size: 0.78rem;
  }

  .card-body,
  .file-stack,
  .file-entry {
    display: grid;
    min-width: 0;
    gap: 0.3rem;
  }

  .file-stack {
    grid-column: 1 / -1;
  }

  .file-heading {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 0.55rem;
    align-items: center;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .thumbnail {
    display: grid;
    place-items: center;
    width: 4.5rem;
    height: 3rem;
    overflow: hidden;
    border-radius: 0.55rem;
    background: var(--ui-control, #eef0f4);
  }

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .image-fallback,
  .unavailable {
    color: var(--ui-danger, #a23838);
    font-size: 0.75rem;
  }

  .pending {
    color: var(--ui-muted, #697181);
    font-size: 0.75rem;
  }

  .favorite {
    position: absolute;
    top: 0.45rem;
    right: 0.55rem;
    color: #b87912;
  }

  @media (max-width: 390px) {
    button {
      padding: 0.65rem;
    }

    .thumbnail {
      width: 3.75rem;
    }
  }
</style>
