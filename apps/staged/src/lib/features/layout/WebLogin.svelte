<!--
  WebLogin.svelte — Token entry screen for web browser clients.

  Shown when the browser has no valid session cookie. The user pastes the
  bearer token displayed in the desktop Tauri app, which is validated
  against the server's /api/auth endpoint.
-->
<script lang="ts">
  import { submitWebToken } from '../../transport';

  let token = $state('');
  let error = $state('');
  let submitting = $state(false);

  async function handleSubmit() {
    error = '';
    submitting = true;
    try {
      const ok = await submitWebToken(token.trim());
      if (ok) {
        // Clear the login hash — the reactive `showLogin` derived state in
        // App.svelte will flip to false and render the main app without a
        // full page reload, preserving any in-memory state.
        window.location.hash = '';
      } else {
        error = 'Invalid token. Check the token shown in the desktop app.';
      }
    } catch {
      error = 'Could not reach the server. Is Staged running?';
    } finally {
      submitting = false;
    }
  }
</script>

<div class="login-container">
  <div class="login-card">
    <h1>Staged</h1>
    <p class="subtitle">Enter the access token from the desktop app to connect.</p>

    <form
      onsubmit={(e) => {
        e.preventDefault();
        handleSubmit();
      }}
    >
      <input
        type="text"
        bind:value={token}
        placeholder="Paste access token"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        disabled={submitting}
      />
      <button type="submit" disabled={submitting || !token.trim()}>
        {submitting ? 'Connecting...' : 'Connect'}
      </button>
    </form>

    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>
</div>

<style>
  .login-container {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    background: var(--bg-primary);
    color: var(--text-primary);
    padding: 1rem;
  }

  .login-card {
    max-width: 400px;
    width: 100%;
    text-align: center;
  }

  h1 {
    font-size: 1.5rem;
    margin: 0 0 0.5rem;
  }

  .subtitle {
    color: var(--text-muted);
    font-size: var(--size-sm);
    margin: 0 0 1.5rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  input {
    padding: 0.625rem 0.75rem;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    background: var(--bg-elevated);
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    outline: none;
  }

  input:focus {
    border-color: var(--text-accent);
  }

  button {
    padding: 0.625rem 0.75rem;
    border: none;
    border-radius: 6px;
    background: var(--text-accent);
    color: #fff;
    font-size: var(--size-md);
    font-family: inherit;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .error {
    color: var(--status-deleted);
    font-size: var(--size-sm);
    margin: 0.75rem 0 0;
  }
</style>
