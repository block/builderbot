<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import FileText from '@lucide/svelte/icons/file-text';
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import Spinner from '../../shared/Spinner.svelte';
  import type { Comment, CommentSessionState, GithubButtonState } from '../../types';

  interface Props {
    comment: Comment;
    noteState: CommentSessionState;
    commitState: CommentSessionState;
    githubState: GithubButtonState;
    hasPr: boolean;
    onNote: (comment: Comment, event: MouseEvent) => void;
    onCommit: (comment: Comment, event: MouseEvent) => void;
    onGithub: (comment: Comment) => void;
  }

  let { comment, noteState, commitState, githubState, hasPr, onNote, onCommit, onGithub }: Props =
    $props();
</script>

<button
  class="comment-action-btn note-btn"
  class:session-active={noteState !== 'idle'}
  onclick={(event) => onNote(comment, event)}
  title={noteState === 'running'
    ? 'Note session in progress'
    : noteState === 'completed'
      ? 'Open note'
      : 'New note (Option+click to skip dialog)'}
>
  {#if noteState === 'running'}
    <Spinner size={12} />
  {:else}
    <FileText size={12} />
  {/if}
  <span>Note</span>
</button>

<button
  class="comment-action-btn commit-btn"
  class:session-active={commitState !== 'idle'}
  onclick={(event) => onCommit(comment, event)}
  title={commitState === 'running'
    ? 'Commit session in progress'
    : commitState === 'completed'
      ? 'Show commit'
      : 'New commit (Option+click to skip dialog)'}
>
  {#if commitState === 'running'}
    <Spinner size={12} />
  {:else}
    <GitCommitVertical size={12} />
  {/if}
  <span>Commit</span>
</button>

{#if hasPr}
  <button
    class="comment-action-btn github-btn"
    class:github-btn-sent={githubState === 'sent'}
    onclick={() => onGithub(comment)}
    title={githubState === 'sent'
      ? 'Open GitHub comment'
      : githubState === 'stale'
        ? 'Update on GitHub'
        : 'Send to GitHub'}
    disabled={githubState === 'sending'}
  >
    {#if githubState === 'sending'}
      <Spinner size={12} />
    {:else if githubState === 'sent'}
      <Check size={12} class="github-sent-check" />
      <GitPullRequest size={12} />
    {:else}
      <GitPullRequest size={12} />
    {/if}
    {#if githubState === 'stale'}
      <span>Update on GitHub</span>
    {:else}
      <span>GitHub</span>
    {/if}
  </button>
{/if}

<style>
  .comment-action-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 6px;
    border-radius: 4px;
    border: 1px dashed var(--border-subtle);
    background: transparent;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .comment-action-btn.note-btn :global(svg) {
    color: var(--note-color);
  }

  .comment-action-btn.commit-btn :global(svg) {
    color: var(--commit-color);
  }

  .comment-action-btn.github-btn :global(svg) {
    color: var(--text-primary);
  }

  .comment-action-btn.note-btn:hover {
    color: var(--note-color);
    border-color: var(--note-color);
    background-color: var(--note-bg);
  }

  .comment-action-btn.commit-btn:hover {
    color: var(--commit-color);
    border-color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  /* Mirrors the styled running/completed note and commit actions in the timeline. */
  .comment-action-btn.note-btn.session-active {
    color: var(--note-color);
    border-color: transparent;
    background-color: var(--note-bg);
  }

  .comment-action-btn.commit-btn.session-active {
    color: var(--commit-color);
    border-color: transparent;
    background-color: var(--commit-bg);
  }

  .comment-action-btn.github-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--text-muted);
    background-color: var(--bg-hover);
  }

  .comment-action-btn.github-btn:disabled {
    cursor: default;
    opacity: 0.7;
  }

  .comment-action-btn.github-btn-sent {
    border-style: solid;
    color: var(--text-primary);
    border-color: var(--status-added);
  }

  .comment-action-btn.github-btn-sent :global(.github-sent-check) {
    color: var(--status-added) !important;
  }
</style>
