<script lang="ts">
  import { onMount } from 'svelte';

  import { clearHistory, getAppInfo } from '../api';
  import { settingsStore } from '../stores/settings';
  import type { AppInfo, AppSettings, ThemeMode } from '../types';

  const BYTES_PER_MEGABYTE = 1024 * 1024;

  interface Props {
    store?: typeof settingsStore;
    clearHistoryAction?: () => Promise<void>;
    getAppInfoAction?: () => Promise<AppInfo>;
    confirmAction?: (message: string) => boolean;
  }

  let {
    store = settingsStore,
    clearHistoryAction = clearHistory,
    getAppInfoAction = getAppInfo,
    confirmAction = (message) => window.confirm(message)
  }: Props = $props();

  let draft = $state<AppSettings | null>(null);
  let maxItemMb = $state(50);
  let lastApplied: AppSettings | null = null;
  let appInfo = $state<AppInfo | null>(null);
  let formError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let saved = $state(false);

  let displayedError = $derived(
    formError ??
      actionError ??
      $store.error ??
      Object.values($store.validationErrors)[0] ??
      null
  );

  $effect(() => {
    if ($store.value && $store.value !== lastApplied) {
      lastApplied = $store.value;
      draft = { ...$store.value };
      maxItemMb = $store.value.maxItemBytes / BYTES_PER_MEGABYTE;
    }
  });

  function errorMessage(error: unknown): string {
    if (
      typeof error === 'object' &&
      error !== null &&
      'message' in error &&
      typeof error.message === 'string'
    ) {
      return error.message;
    }
    return error instanceof Error ? error.message : String(error);
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    saved = false;
    formError = null;
    actionError = null;
    if (
      !draft ||
      !Number.isInteger(maxItemMb) ||
      maxItemMb < 1 ||
      maxItemMb > 500
    ) {
      formError = '单次内容上限必须是 1–500 MB 的整数';
      return;
    }
    const requested: AppSettings = {
      ...draft,
      maxItemBytes: maxItemMb * BYTES_PER_MEGABYTE
    };
    try {
      await store.save(requested);
      if (Object.keys($store.validationErrors).length === 0) {
        saved = true;
      }
    } catch {
      // Native registration is transactional. Reflect its hotkey rollback without
      // discarding the user's other unsaved field edits.
      if (draft && $store.value) {
        draft = { ...draft, hotkey: $store.value.hotkey };
      }
    }
  }

  async function clearNormalHistory(): Promise<void> {
    actionError = null;
    if (!confirmAction('确定清空所有普通历史？收藏内容会保留。')) {
      return;
    }
    try {
      await clearHistoryAction();
    } catch (error) {
      actionError = errorMessage(error);
    }
  }

  onMount(() => {
    void store.ensureLoaded().catch(() => undefined);
    void getAppInfoAction()
      .then((value) => (appInfo = value))
      .catch((error) => (actionError = errorMessage(error)));
  });
</script>

<main class="settings-view">
  <header class="settings-header">
    <div>
      <p class="eyebrow">Easy Clipboard</p>
      <h1>设置</h1>
    </div>
    <span>偏好会同步到剪贴板窗口</span>
  </header>

  {#if $store.loading && !draft}
    <p class="state" role="status">正在读取设置…</p>
  {:else if draft}
    <form novalidate onsubmit={submit}>
      <section class="settings-card" aria-labelledby="behavior-title">
        <div class="section-heading">
          <span class="section-icon" aria-hidden="true">⌘</span>
          <div>
            <h2 id="behavior-title">剪贴板行为</h2>
            <p>控制呼出方式和单条内容容量。</p>
          </div>
        </div>

        <label class="field">
          <span>全局热键</span>
          <input
            aria-label="全局热键"
            type="text"
            bind:value={draft.hotkey}
            autocomplete="off"
          />
        </label>

        <label class="field">
          <span>单次内容上限 (MB)</span>
          <input
            aria-label="单次内容上限 (MB)"
            type="number"
            min="1"
            max="500"
            step="1"
            bind:value={maxItemMb}
          />
          <small>范围 1–500 MB，默认 50 MB</small>
        </label>
      </section>

      <section class="settings-card" aria-labelledby="appearance-title">
        <div class="section-heading">
          <span class="section-icon" aria-hidden="true">◐</span>
          <div>
            <h2 id="appearance-title">外观与动画</h2>
            <p>选择 Aurora 浅色、Ink 深色或跟随系统。</p>
          </div>
        </div>

        <label class="field">
          <span>主题</span>
          <select aria-label="主题" bind:value={draft.theme}>
            <option value="system">跟随系统</option>
            <option value="light">Aurora 浅色</option>
            <option value="dark">Ink 深色</option>
          </select>
        </label>

        <label class="field">
          <span>动画强度</span>
          <div class="range-row">
            <input
              aria-label="动画强度"
              type="range"
              min="0"
              max="2"
              step="0.05"
              bind:value={draft.motionScale}
            />
            <output>{draft.motionScale.toFixed(2)}×</output>
          </div>
        </label>
      </section>

      <section class="settings-card" aria-labelledby="startup-title">
        <div class="section-heading">
          <span class="section-icon" aria-hidden="true">↗</span>
          <div>
            <h2 id="startup-title">启动与隐私</h2>
            <p>决定应用何时启动以及退出时保留什么。</p>
          </div>
        </div>

        <label class="toggle-row">
          <span>
            <strong>开机启动</strong>
            <small>登录 Windows 后自动运行</small>
          </span>
          <input
            aria-label="开机启动"
            type="checkbox"
            bind:checked={draft.autostart}
          />
        </label>
        <label class="toggle-row">
          <span>
            <strong>退出时清空普通历史</strong>
            <small>收藏内容仍会保留</small>
          </span>
          <input
            aria-label="退出时清空普通历史"
            type="checkbox"
            bind:checked={draft.clearOnExit}
          />
        </label>
      </section>

      {#if displayedError}
        <p class="message error" role="alert">{displayedError}</p>
      {:else if saved}
        <p class="message success" role="status">设置已保存</p>
      {/if}

      <div class="form-actions">
        <button
          class="danger-button"
          type="button"
          onclick={clearNormalHistory}
        >
          清空普通历史
        </button>
        <button class="primary-button" type="submit" disabled={$store.saving}>
          {$store.saving ? '正在保存…' : '保存设置'}
        </button>
      </div>
    </form>
  {:else if $store.error}
    <p class="state error" role="alert">{$store.error}</p>
  {/if}

  <footer class="app-info">
    {#if appInfo}
      <span>版本 {appInfo.version}</span>
      <span class="data-path" title={appInfo.appDataDir}>{appInfo.appDataDir}</span>
    {:else}
      <span>正在读取应用信息…</span>
    {/if}
  </footer>
</main>
