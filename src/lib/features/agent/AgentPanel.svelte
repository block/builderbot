<!--
  AgentPanel.svelte - AI agent chat interface
  
  Provides a simple chat input for asking questions about the current diff/changeset.
  Maintains session state for multi-turn conversations.
  
  Each tab has its own AgentState, passed as a prop to ensure chat sessions are isolated.
-->
<script lang="ts">
  import { Send, Bot, Loader2, ChevronDown } from 'lucide-svelte';
  import { sendAgentPrompt, discoverAcpProviders, type AcpProviderInfo } from '../../services/ai';
  import { agentGlobalState, type AcpProvider, type AgentState } from '../../stores/agent.svelte';
  import type { FileDiffSummary } from '../../types';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';

  import { onMount } from 'svelte';

  // Configure marked for safe rendering
  marked.setOptions({
    breaks: true,
    gfm: true,
  });

  interface Props {
    /** Repository path for AI agent */
    repoPath?: string | null;
    /** File summaries from the current diff */
    files?: FileDiffSummary[];
    /** Currently selected file path */
    selectedFile?: string | null;
    /** Agent state for this tab's chat session (required) */
    agentState: AgentState;
  }

  let { repoPath = null, files = [], selectedFile = null, agentState }: Props = $props();

  let showProviderDropdown = $state(false);

  /** Type guard to validate provider ID */
  function isValidProvider(id: string): id is AcpProvider {
    return id === 'goose' || id === 'claude';
  }

  // Parse markdown response and sanitize to prevent XSS
  let renderedResponse = $derived(
    agentState.response ? DOMPurify.sanitize(marked.parse(agentState.response) as string) : ''
  );

  // Initialize on mount: discover providers and set up click-outside handler
  onMount(() => {
    // Discover available providers (only once globally)
    if (!agentGlobalState.providersLoaded) {
      discoverAcpProviders()
        .then((providers) => {
          agentGlobalState.availableProviders = providers;
          agentGlobalState.providersLoaded = true;

          // If current provider is not available, switch to first available valid one
          if (providers.length > 0 && !providers.some((p) => p.id === agentState.provider)) {
            const firstValidId = providers.map((p) => p.id).find(isValidProvider);
            if (firstValidId) {
              agentState.provider = firstValidId;
            }
          }
        })
        .catch((e) => {
          console.error('Failed to discover ACP providers:', e);
        });
    }

    // Close dropdown when clicking outside
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as HTMLElement;
      if (showProviderDropdown && !target.closest('.provider-picker')) {
        showProviderDropdown = false;
      }
    }
    document.addEventListener('click', handleClickOutside);
    return () => document.removeEventListener('click', handleClickOutside);
  });

  function selectProvider(provider: AcpProvider) {
    agentState.provider = provider;
    showProviderDropdown = false;
    // Reset session when switching providers
    agentState.sessionId = null;
    agentState.response = '';
    agentState.error = '';
  }

  function toggleProviderDropdown() {
    showProviderDropdown = !showProviderDropdown;
  }

  /**
   * Get the primary path for a file summary.
   */
  function getFilePath(summary: FileDiffSummary): string {
    return summary.after ?? summary.before ?? '';
  }

  /**
   * Build context-aware prompt with file information.
   */
  function buildPromptWithContext(userPrompt: string, isNewSession: boolean): string {
    let context = '';

    // For new sessions, include changeset overview (up to 5 files)
    if (isNewSession && files.length > 0) {
      const fileNames = files.slice(0, 5).map((f) => getFilePath(f));
      const moreCount = files.length > 5 ? ` (+${files.length - 5} more)` : '';
      context += `[Changeset: ${fileNames.join(', ')}${moreCount}]\n`;
    }

    // Always include current file context
    if (selectedFile) {
      context += `[Viewing: ${selectedFile}]\n`;
    }

    return context ? context + '\n' + userPrompt : userPrompt;
  }

  /**
   * Send prompt to AI agent.
   * Captures the agentState reference at call time to ensure responses go to the correct tab.
   */
  async function handleSubmit() {
    const userPrompt = agentState.input.trim();
    if (!userPrompt || agentState.loading) return;

    // Capture reference to this tab's state - ensures async response goes to correct tab
    const tabState = agentState;

    tabState.loading = true;
    tabState.error = '';
    tabState.response = '';
    const inputToSend = tabState.input;
    tabState.input = '';

    try {
      const isNewSession = !tabState.sessionId;
      const promptWithContext = buildPromptWithContext(inputToSend, isNewSession);
      const result = await sendAgentPrompt(
        repoPath,
        promptWithContext,
        tabState.sessionId,
        tabState.provider
      );
      // Write response to the captured tab state, not the current prop
      tabState.response = result.response;
      tabState.sessionId = result.sessionId;
    } catch (e) {
      tabState.error = e instanceof Error ? e.message : String(e);
    } finally {
      tabState.loading = false;
    }
  }

  /**
   * Handle Enter key in input.
   */
  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      handleSubmit();
    }
  }

  let textareaEl: HTMLTextAreaElement | null = $state(null);

  /**
   * Auto-resize textarea to fit content, up to max rows.
   */
  function autoResize() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = textareaEl.scrollHeight + 'px';
  }

  $effect(() => {
    // Re-run when input changes
    agentState.input;
    autoResize();
  });
</script>

<div class="agent-section">
  <div class="agent-top">
    {#if agentState.error}
      <div class="agent-error">
        {agentState.error}
      </div>
    {/if}
    {#if agentState.loading || agentState.response}
      <div class="agent-response">
        <div class="agent-response-header">
          <Bot size={12} />
          <span>Agent</span>
        </div>
        <div class="agent-response-content" class:loading={agentState.loading}>
          {#if agentState.loading}
            <Loader2 size={14} class="spinning" /> Thinking...
          {:else}
            <div class="markdown-content">
              {@html renderedResponse}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
  <div class="agent-bottom">
    <div class="agent-input-wrapper">
      <textarea
        class="agent-input"
        placeholder="Ask the agent..."
        bind:value={agentState.input}
        bind:this={textareaEl}
        onkeydown={handleKeydown}
        disabled={agentState.loading}
        rows="1"
      ></textarea>
      <div class="agent-input-actions">
        {#if agentGlobalState.availableProviders.length > 0}
          <div class="provider-picker">
            <button
              class="provider-btn"
              onclick={toggleProviderDropdown}
              disabled={agentState.loading}
              title="Select AI provider"
            >
              <span class="provider-label"
                >{agentGlobalState.availableProviders.find((p) => p.id === agentState.provider)
                  ?.label ?? agentState.provider}</span
              >
              <ChevronDown size={12} />
            </button>
            {#if showProviderDropdown}
              <div class="provider-dropdown">
                {#each agentGlobalState.availableProviders as provider (provider.id)}
                  <button
                    class="provider-option"
                    class:selected={agentState.provider === provider.id}
                    onclick={() => isValidProvider(provider.id) && selectProvider(provider.id)}
                  >
                    {provider.label}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
        <button
          class="agent-send-btn"
          onclick={handleSubmit}
          disabled={agentState.loading || !agentState.input.trim()}
          title="Send to agent"
        >
          {#if agentState.loading}
            <Loader2 size={14} class="spinning" />
          {:else}
            <Send size={14} />
          {/if}
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .agent-section {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    padding: 0 12px;
  }

  .agent-top {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .agent-bottom {
    flex-shrink: 0;
    padding: 12px 0;
  }

  .agent-input-wrapper {
    display: flex;
    flex-direction: column;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    padding: 10px 12px 8px;
    transition: border-color 0.1s;
  }

  .agent-input-wrapper:focus-within {
    border-color: var(--text-accent);
  }

  .agent-input {
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    padding: 0;
    outline: none;
    min-width: 0;
    resize: none;
    line-height: 1.4;
    overflow-y: auto;
    max-height: calc(1.4em * 4);
  }

  .agent-input::placeholder {
    color: var(--text-faint);
  }

  .agent-input:disabled {
    opacity: 0.6;
  }

  .agent-input-actions {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 6px;
  }

  /* Provider picker */
  .provider-picker {
    position: relative;
  }

  .provider-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 6px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .provider-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-muted);
  }

  .provider-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .provider-label {
    white-space: nowrap;
  }

  .provider-dropdown {
    position: absolute;
    bottom: 100%;
    left: 0;
    margin-bottom: 4px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
    z-index: 100;
    min-width: 120px;
  }

  .provider-option {
    display: block;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .provider-option:hover {
    background-color: var(--bg-hover);
  }

  .provider-option.selected {
    background-color: var(--bg-primary);
    color: var(--text-accent);
  }

  .agent-send-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
    flex-shrink: 0;
  }

  .agent-send-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-accent);
  }

  .agent-send-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .agent-send-btn :global(.spinning) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .agent-error {
    margin-bottom: 8px;
    padding: 8px;
    background: var(--ui-danger-bg);
    border-radius: 4px;
    color: var(--ui-danger);
    font-size: var(--size-xs);
    word-break: break-word;
  }

  .agent-response {
    margin-bottom: 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    overflow: hidden;
  }

  .agent-response-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    background: var(--bg-hover);
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
  }

  .agent-response-content {
    padding: 10px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    word-break: break-word;
    max-height: 200px;
    overflow-y: auto;
  }

  .agent-response-content.loading {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-style: italic;
  }

  /* Markdown content styles */
  .markdown-content :global(p) {
    margin: 0 0 0.5em 0;
  }

  .markdown-content :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3),
  .markdown-content :global(h4) {
    margin: 0.75em 0 0.5em 0;
    font-weight: 600;
    line-height: 1.3;
  }

  .markdown-content :global(h1:first-child),
  .markdown-content :global(h2:first-child),
  .markdown-content :global(h3:first-child),
  .markdown-content :global(h4:first-child) {
    margin-top: 0;
  }

  .markdown-content :global(h1) {
    font-size: 1.25em;
  }

  .markdown-content :global(h2) {
    font-size: 1.15em;
  }

  .markdown-content :global(h3) {
    font-size: 1.05em;
  }

  .markdown-content :global(ul),
  .markdown-content :global(ol) {
    margin: 0.5em 0;
    padding-left: 1.5em;
  }

  .markdown-content :global(li) {
    margin: 0.25em 0;
  }

  .markdown-content :global(code) {
    font-family: var(--font-mono);
    font-size: 0.9em;
    background: var(--bg-hover);
    padding: 0.15em 0.35em;
    border-radius: 3px;
  }

  .markdown-content :global(pre) {
    margin: 0.5em 0;
    padding: 0.75em;
    background: var(--bg-hover);
    border-radius: 4px;
    overflow-x: auto;
  }

  .markdown-content :global(pre code) {
    background: none;
    padding: 0;
    font-size: 0.85em;
  }

  .markdown-content :global(blockquote) {
    margin: 0.5em 0;
    padding-left: 0.75em;
    border-left: 3px solid var(--border-muted);
    color: var(--text-muted);
  }

  .markdown-content :global(a) {
    color: var(--text-accent);
    text-decoration: none;
  }

  .markdown-content :global(a:hover) {
    text-decoration: underline;
  }

  .markdown-content :global(strong) {
    font-weight: 600;
  }

  .markdown-content :global(hr) {
    margin: 0.75em 0;
    border: none;
    border-top: 1px solid var(--border-subtle);
  }
</style>
