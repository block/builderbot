<script lang="ts">
  import { BitsConfig } from 'bits-ui';
  import App from './App.svelte';
</script>

<!-- Every bits-ui portal lands in #portal-root (index.html, ensured by
     main.ts) instead of <body>, so opening or closing one never changes
     #app's sibling position and restyles the whole app. -->
<BitsConfig defaultPortalTo="#portal-root">
  <App />
</BitsConfig>
