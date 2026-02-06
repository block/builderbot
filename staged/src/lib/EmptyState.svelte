<!--
  EmptyState.svelte - Empty state for when there are no changes
  
  Shows a generative git tree animation with branches growing,
  providing visual delight while communicating "nothing to review".
-->
<script lang="ts">
  import GitTreeAnimation from './GitTreeAnimation.svelte';
</script>

<div class="empty-state-container">
  <GitTreeAnimation />
  <div class="message">
    <p class="title">No changes to review</p>
    <p class="subtitle">Select a different changeset, or chat with the agent to start working</p>
  </div>
</div>

<style>
  .empty-state-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    width: 100%;
    gap: 24px;
  }

  .message {
    text-align: center;
  }

  .title {
    font-size: var(--size-xl);
    font-weight: 500;
    color: var(--text-primary);
    margin: 0 0 8px 0;
  }

  .subtitle {
    font-size: var(--size-md);
    color: var(--text-muted);
    margin: 0;
  }
</style>
