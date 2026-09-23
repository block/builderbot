<!--
  AgentLoginPrompt.svelte — the user-facing half of an interactive agent login.

  Shows the sign-in URL the CLI printed (the CLI's own browser launch can fail
  silently, and a web-server client never had a browser on the host), a box for
  the code the sign-in page hands back, a tail of the fix's output so a login
  that is waiting on something else isn't a bare spinner, and a Cancel — the CLI
  ignores a closed stdin once it is waiting on its browser callback, so without
  one an abandoned login holds the check's login slot until doctor's fix timeout.

  Renders nothing unless the shared record in `agentLogin.svelte.ts` belongs to
  `checkId`, so it can be dropped beside any check without a guard. A host whose
  own chrome already offers the cancel (the Doctor panel's fix dialog) passes
  `cancellable={false}` rather than showing two.
-->
<script lang="ts">
  import { openUrl } from '../../api/commands';
  import { Button } from '$lib/components/ui/button';
  import {
    agentLogin,
    agentLoginFor,
    cancelAgentLogin,
    submitAgentLoginCode,
  } from './agentLogin.svelte';

  let {
    checkId,
    cancellable = true,
  }: { checkId: string | null | undefined; cancellable?: boolean } = $props();

  const login = $derived(agentLoginFor(checkId));
</script>

{#if login}
  {#if login.running}
    <div class="login-prompt">
      {#if login.url}
        {@const url = login.url}
        <p class="login-hint">Sign in, then paste the code the page gives you:</p>
        <div class="login-url-row">
          <code class="login-url">{url}</code>
          <Button variant="outline" size="xs" onclick={() => openUrl(url)}>Open</Button>
        </div>
      {:else}
        <p class="login-hint">Starting sign-in…</p>
      {/if}
      <div class="login-code-row">
        <input
          class="login-code-input"
          aria-label="Authentication code"
          placeholder="Paste authentication code"
          bind:value={agentLogin.code}
          onkeydown={(event) => event.key === 'Enter' && submitAgentLoginCode()}
        />
        <Button
          variant="outline"
          size="xs"
          onclick={submitAgentLoginCode}
          disabled={!agentLogin.code.trim() || login.sending}
        >
          {login.sending ? 'Sending…' : 'Submit code'}
        </Button>
        {#if cancellable}
          <Button variant="ghost" size="xs" onclick={cancelAgentLogin} disabled={login.cancelling}>
            {login.cancelling ? 'Cancelling…' : 'Cancel'}
          </Button>
        {/if}
      </div>
      {#if login.output.length > 0}
        <pre class="login-output">{login.output.join('\n')}</pre>
      {/if}
    </div>
  {/if}
  {#if login.error}
    <p class="login-error">{login.error}</p>
  {/if}
{/if}

<style>
  .login-prompt {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 8px;
  }

  .login-hint {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .login-url-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .login-url {
    flex: 1;
    min-width: 0;
    font-family: monospace;
    font-size: 10px;
    color: var(--text-muted);
    overflow-wrap: anywhere;
    /* Selectable: a user reading this over a remote session copies it by hand. */
    user-select: text;
  }

  .login-code-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .login-code-input {
    min-width: 180px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    padding: 4px 8px;
    font-size: var(--size-xs);
  }

  .login-output {
    max-height: 120px;
    overflow: auto;
    margin: 0;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--bg-primary);
    font-family: monospace;
    font-size: 10px;
    color: var(--text-muted);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }

  .login-error {
    font-size: var(--size-xs);
    color: var(--color-danger, #f85149);
  }
</style>
