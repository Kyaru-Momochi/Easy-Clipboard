<script lang="ts">
  import AppShell from './lib/components/AppShell.svelte';
</script>

<svelte:head>
  <title>Easy Clipboard</title>
</svelte:head>

<AppShell />

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(html),
  :global(body),
  :global(#app) {
    width: 100%;
    height: 100%;
    margin: 0;
  }

  :global(body) {
    overflow: hidden;
    background: #fafbfc;
  }
</style>
