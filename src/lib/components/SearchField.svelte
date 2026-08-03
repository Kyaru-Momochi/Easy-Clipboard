<script lang="ts">
  interface Props {
    value: string;
    onSearch: (value: string) => void;
  }

  let { value, onSearch }: Props = $props();

  function forwardValue(event: Event): void {
    onSearch((event.currentTarget as HTMLInputElement).value);
  }
</script>

<label class="search-field">
  <span aria-hidden="true" class="search-icon">⌕</span>
  <input
    type="search"
    aria-label="搜索剪贴板历史"
    placeholder="搜索剪贴板历史"
    autocomplete="off"
    spellcheck="false"
    {value}
    oninput={forwardValue}
  />
</label>

<style>
  .search-field {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.55rem;
    min-height: 2.6rem;
    padding: 0 0.8rem;
    border: 1px solid var(--ui-border);
    border-radius: var(--radius-md);
    background: var(--ui-control);
  }

  .search-field:focus-within {
    border-color: var(--ui-accent);
    outline: 2px solid color-mix(in srgb, var(--ui-accent) 25%, transparent);
  }

  .search-icon {
    color: var(--ui-muted);
    font-size: 1.15rem;
  }

  input {
    width: 100%;
    min-width: 0;
    border: 0;
    outline: 0;
    color: inherit;
    background: transparent;
    font: inherit;
  }
</style>
