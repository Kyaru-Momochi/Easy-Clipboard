<script lang="ts">
  import { onMount } from 'svelte';

  import './app.css';
  import AppShell from './lib/components/AppShell.svelte';
  import SettingsView from './lib/components/SettingsView.svelte';
  import { settingsStore } from './lib/stores/settings';
  import { createThemeController } from './lib/theme';

  const isSettingsView =
    new URLSearchParams(window.location.search).get('view') === 'settings';

  onMount(() => createThemeController({ store: settingsStore }).mount());
</script>

<svelte:head>
  <title>{isSettingsView ? 'Easy Clipboard 设置' : 'Easy Clipboard'}</title>
</svelte:head>

{#if isSettingsView}
  <SettingsView />
{:else}
  <AppShell />
{/if}
