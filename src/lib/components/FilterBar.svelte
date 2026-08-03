<script lang="ts">
  import type { HistoryFilter } from '../types';

  interface Props {
    value: HistoryFilter['kind'];
    onChange: (value: HistoryFilter['kind']) => void;
  }

  const filters = [
    { value: 'all', label: '全部' },
    { value: 'text', label: '文本' },
    { value: 'image', label: '图片' },
    { value: 'files', label: '文件' }
  ] as const;

  let { value, onChange }: Props = $props();
</script>

<div class="filter-bar" role="group" aria-label="剪贴板类型筛选">
  {#each filters as filter}
    <button
      type="button"
      aria-pressed={value === filter.value}
      onclick={() => onChange(filter.value)}
    >
      {filter.label}
    </button>
  {/each}
</div>

<style>
  .filter-bar {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0.35rem;
  }

  button {
    min-height: 2.1rem;
    border: 1px solid transparent;
    border-radius: 0.65rem;
    color: var(--ui-muted, #697181);
    background: transparent;
    font: inherit;
    cursor: pointer;
  }

  button:hover {
    background: var(--ui-control, #f0f2f6);
  }

  button[aria-pressed='true'] {
    border-color: var(--ui-border, #d7dce5);
    color: var(--ui-accent, #4057bd);
    background: var(--ui-control, #eef1fb);
    font-weight: 650;
  }

  button:focus-visible {
    outline: 2px solid var(--ui-accent, #5068d8);
    outline-offset: 1px;
  }
</style>
