<!--
  AgentPanel.svelte - AI agent chat interface
  
  Provides a simple chat input for asking questions about the current diff/changeset.
  Maintains session state for multi-turn conversations.
  Supports saving responses as artifacts for later reference.
  Artifacts are persisted to the database and restored on mount.
  
  Each tab has its own AgentState, passed as a prop to ensure chat sessions are isolated.
-->
<script lang="ts">
  import {
    Send,
    Bot,
    Loader2,
    ChevronDown,
    ChevronRight,
    Save,
    FileText,
    X,
    Trash2,
    Circle,
    CheckCircle2,
    MessageSquare,
  } from 'lucide-svelte';
  import {
    sendAgentPromptStreaming,
    discoverAcpProviders,
    type AcpProviderInfo,
  } from '../../services/ai';
  import { liveSessionStore } from '../../stores/liveSession.svelte';
  import LiveSessionView from '../../LiveSessionView.svelte';
  import {
    saveArtifact,
    getArtifacts,
    deleteArtifactFromDb,
    type Artifact,
  } from '../../services/review';
  import {
    agentGlobalState,
    generateArtifactId,
    type AcpProvider,
    type AgentState,
  } from '../../stores/agent.svelte';
  import { commentsState } from '../../stores/comments.svelte';
  import { preferences } from '../../stores/preferences.svelte';
  import type { DiffSpec, FileDiffSummary } from '../../types';
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
    /** Current diff spec for artifact persistence */
    spec?: DiffSpec | null;
    /** File summaries from the current diff */
    files?: FileDiffSummary[];
    /** Currently selected file path */
    selectedFile?: string | null;
    /** Agent state for this tab's chat session (required) */
    agentState: AgentState;
  }

  let {
    repoPath = null,
    spec = null,
    files = [],
    selectedFile = null,
    agentState,
  }: Props = $props();

  let showProviderDropdown = $state(false);
  let responseExpanded = $state(true);
  let confirmingDeleteId = $state<string | null>(null);
  let artifactsLoaded = $state(false);
  let loadingArtifacts = $state(false);

  // Modal state for viewing artifact content
  let viewingArtifact = $state<Artifact | null>(null);

  // Track which artifacts are selected for context in the next session
  let selectedArtifactIds = $state<Set<string>>(new Set());

  // Track whether to include all comments in context (default: true)
  let includeCommentsInContext = $state(true);

  // Count total comments for display
  let totalCommentsCount = $derived(commentsState.comments.length);

  /** Type guard to validate provider ID */
  function isValidProvider(id: string): id is AcpProvider {
    return id === 'goose' || id === 'claude' || id === 'codex';
  }

  // Parse markdown response and sanitize to prevent XSS
  let renderedResponse = $derived(
    agentState.response ? DOMPurify.sanitize(marked.parse(agentState.response) as string) : ''
  );

  // Get the live session for streaming display
  // When loading, we may not have the session ID yet, so check most recent streaming session
  let liveSession = $derived.by(() => {
    // If we have a session ID, look it up directly
    if (agentState.sessionId) {
      const session = liveSessionStore.get(agentState.sessionId);
      if (session?.isStreaming) return session;
    }
    // If loading but no session ID yet, check for most recent streaming session
    if (agentState.loading) {
      return liveSessionStore.getMostRecentStreaming();
    }
    return undefined;
  });

  // Check if we're actively streaming (have a live session that's still streaming)
  let isStreaming = $derived(liveSession?.isStreaming ?? false);

  /**
   * Load artifacts from the database for the current diff spec.
   */
  async function loadArtifactsFromDb() {
    if (!spec || artifactsLoaded || loadingArtifacts) return;

    loadingArtifacts = true;
    try {
      const artifacts = await getArtifacts(spec, repoPath ?? undefined);
      // Replace the artifacts array with loaded ones
      agentState.artifacts.length = 0;
      agentState.artifacts.push(...artifacts);
      // Default all loaded artifacts to selected
      selectedArtifactIds = new Set(artifacts.map((a) => a.id));
      artifactsLoaded = true;
    } catch (e) {
      console.error('Failed to load artifacts:', e);
    } finally {
      loadingArtifacts = false;
    }
  }

  // Initialize on mount: discover providers, load artifacts, and set up click-outside handler
  onMount(() => {
    // Discover available providers (only once globally)
    if (!agentGlobalState.providersLoaded) {
      discoverAcpProviders()
        .then((providers) => {
          agentGlobalState.availableProviders = providers;
          agentGlobalState.providersLoaded = true;

          // Use saved preference if available, otherwise fall back to first available
          const savedAgent = preferences.aiAgent;
          if (
            savedAgent &&
            providers.some((p) => p.id === savedAgent) &&
            isValidProvider(savedAgent)
          ) {
            agentState.provider = savedAgent as AcpProvider;
          } else if (providers.length > 0 && !providers.some((p) => p.id === agentState.provider)) {
            // If current provider is not available, switch to first available valid one
            const firstValidId = providers.map((p) => p.id).find(isValidProvider);
            if (firstValidId) {
              agentState.provider = firstValidId;
            }
          }
        })
        .catch((e) => {
          console.error('Failed to discover ACP providers:', e);
        });
    } else {
      // Providers already loaded - check if we should use the saved preference
      const savedAgent = preferences.aiAgent;
      if (
        savedAgent &&
        agentGlobalState.availableProviders.some((p) => p.id === savedAgent) &&
        isValidProvider(savedAgent)
      ) {
        agentState.provider = savedAgent as AcpProvider;
      }
    }

    // Load artifacts from database
    loadArtifactsFromDb();

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

  // Track the spec string to detect actual changes (not just object reference changes)
  let lastSpecKey = $state<string | null>(null);

  // Reload artifacts when spec actually changes (comparing by value, not reference)
  $effect(() => {
    if (!spec) return;

    // Create a stable key from the spec to detect actual changes
    const specKey = `${spec.base.type}:${spec.base.type === 'Rev' ? spec.base.value : ''}::${spec.head.type}:${spec.head.type === 'Rev' ? spec.head.value : ''}`;

    if (specKey !== lastSpecKey) {
      lastSpecKey = specKey;
      artifactsLoaded = false;
      loadArtifactsFromDb();
    }
  });

  // Sync provider when preferences.aiAgent changes (e.g., from Settings modal)
  $effect(() => {
    const savedAgent = preferences.aiAgent;
    if (
      savedAgent &&
      agentGlobalState.availableProviders.some((p) => p.id === savedAgent) &&
      isValidProvider(savedAgent)
    ) {
      agentState.provider = savedAgent as AcpProvider;
    }
  });

  // Auto-select new artifacts added externally (e.g., from AI analysis in Sidebar)
  $effect(() => {
    const artifactIds = agentState.artifacts.map((a) => a.id);
    let added = false;
    for (const id of artifactIds) {
      if (!selectedArtifactIds.has(id)) {
        selectedArtifactIds.add(id);
        added = true;
      }
    }
    // Trigger reactivity if we added any
    if (added) {
      selectedArtifactIds = new Set(selectedArtifactIds);
    }
  });

  function selectProvider(provider: AcpProvider) {
    agentState.provider = provider;
    showProviderDropdown = false;
    // Reset session when switching providers
    agentState.sessionId = null;
    agentState.task = null;
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
   * For follow-up messages, includes the original task to keep the agent focused.
   * For new sessions, includes selected artifacts and comments as context.
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

    // For new sessions, include selected artifacts as context
    if (isNewSession && selectedArtifactIds.size > 0) {
      const selectedArtifacts = agentState.artifacts.filter((a) => selectedArtifactIds.has(a.id));
      if (selectedArtifacts.length > 0) {
        context += '\n[Reference artifacts:]\n';
        for (const artifact of selectedArtifacts) {
          context += `\n--- ${artifact.title} ---\n${artifact.content}\n`;
        }
        context += '\n';
      }
    }

    // For new sessions, include all comments if selected
    if (isNewSession && includeCommentsInContext && commentsState.comments.length > 0) {
      context += '\n[Code Comments from Review:]\n';
      context += 'Here are comments left on the code during review:\n\n';

      // Group comments by file
      const commentsByFile = new Map<string, typeof commentsState.comments>();
      for (const comment of commentsState.comments) {
        if (!commentsByFile.has(comment.path)) {
          commentsByFile.set(comment.path, []);
        }
        commentsByFile.get(comment.path)!.push(comment);
      }

      // Format comments by file
      for (const [filePath, comments] of commentsByFile) {
        context += `File: ${filePath}\n`;
        for (const comment of comments) {
          const lineInfo =
            comment.span.end - comment.span.start === 1
              ? `line ${comment.span.start + 1}`
              : `lines ${comment.span.start + 1}-${comment.span.end}`;

          context += `  @ ${lineInfo}: ${comment.content}\n`;
        }
        context += '\n';
      }

      context += 'Use these comments to understand concerns and feedback about the code.\n\n';
    }

    // For follow-up messages, remind the agent of the original task
    if (!isNewSession && agentState.task) {
      context += `[Original task: ${agentState.task}]\n`;
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

    const isNewSession = !tabState.sessionId;

    // Store the original task on new sessions
    if (isNewSession) {
      tabState.task = userPrompt;
    }

    tabState.loading = true;
    tabState.error = '';
    tabState.response = '';
    responseExpanded = true; // Keep expanded to show streaming
    const inputToSend = tabState.input;
    tabState.input = '';

    try {
      const promptWithContext = buildPromptWithContext(inputToSend, isNewSession);

      // Use streaming API - events will be handled by liveSessionStore
      const result = await sendAgentPromptStreaming(promptWithContext, {
        repoPath: repoPath ?? undefined,
        sessionId: tabState.sessionId ?? undefined,
        provider: tabState.provider,
      });

      // Write final response to the captured tab state
      tabState.response = result.response;
      tabState.sessionId = result.sessionId;

      // Clear the live session now that we have the final response
      liveSessionStore.clear(result.sessionId);
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

  /**
   * Save the current response as an artifact.
   * Keeps the session alive so user can continue the conversation.
   */
  async function saveAsArtifact() {
    if (!agentState.response || !spec) return;

    // Generate title from first line of response (strip markdown)
    const firstLine = agentState.response.split('\n')[0] || 'Untitled';
    const title = firstLine.replace(/^#+\s*/, '').slice(0, 50);

    const artifact: Artifact = {
      id: generateArtifactId(),
      title,
      content: agentState.response,
      createdAt: new Date().toISOString(),
    };

    // Add to local state immediately for responsiveness
    agentState.artifacts.push(artifact);
    // Default new artifacts to selected
    selectedArtifactIds.add(artifact.id);
    selectedArtifactIds = new Set(selectedArtifactIds); // Trigger reactivity
    agentState.response = ''; // Clear response after saving (session stays alive)

    // Persist to database (fire-and-forget, errors logged)
    try {
      await saveArtifact(spec, artifact, repoPath ?? undefined);
    } catch (e) {
      console.error('Failed to save artifact to database:', e);
    }
  }

  /**
   * Discard the current response and end the session.
   * Use this when the response isn't what you want and you want to start fresh.
   */
  function discardResponse() {
    agentState.response = '';
    agentState.sessionId = null; // End the session
    agentState.task = null; // Clear the original task
    agentState.error = '';
  }

  /**
   * Delete an artifact.
   */
  async function deleteArtifact(id: string) {
    const index = agentState.artifacts.findIndex((a) => a.id === id);
    if (index !== -1) {
      agentState.artifacts.splice(index, 1);
      // Close modal if viewing this artifact
      if (viewingArtifact?.id === id) {
        viewingArtifact = null;
      }
      // Remove from selection if selected
      if (selectedArtifactIds.has(id)) {
        selectedArtifactIds.delete(id);
        selectedArtifactIds = new Set(selectedArtifactIds); // Trigger reactivity
      }

      // Delete from database (fire-and-forget, errors logged)
      try {
        await deleteArtifactFromDb(id);
      } catch (e) {
        console.error('Failed to delete artifact from database:', e);
      }
    }
  }

  /**
   * Toggle artifact selection for context inclusion.
   */
  function toggleArtifactSelection(id: string) {
    if (selectedArtifactIds.has(id)) {
      selectedArtifactIds.delete(id);
    } else {
      selectedArtifactIds.add(id);
    }
    selectedArtifactIds = new Set(selectedArtifactIds); // Trigger reactivity
  }

  /**
   * Toggle comments inclusion in context.
   */
  function toggleCommentsInContext() {
    includeCommentsInContext = !includeCommentsInContext;
  }

  /**
   * Open artifact viewer modal.
   */
  function openArtifactViewer(artifact: Artifact) {
    viewingArtifact = artifact;
  }

  /**
   * Close artifact viewer modal.
   */
  function closeArtifactViewer() {
    viewingArtifact = null;
  }

  /**
   * Handle keydown on modal backdrop for Escape to close.
   */
  function handleModalKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      closeArtifactViewer();
    }
  }

  /**
   * Render artifact content as HTML.
   */
  function renderArtifactContent(content: string): string {
    return DOMPurify.sanitize(marked.parse(content) as string);
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
  <!-- Top area: errors and saved artifacts (scrollable) -->
  <div class="agent-top">
    {#if agentState.error}
      <div class="agent-error">
        {agentState.error}
      </div>
    {/if}

    <!-- Saved artifacts list -->
    {#each agentState.artifacts as artifact (artifact.id)}
      <div class="artifact-item">
        <div
          class="artifact-header"
          role="button"
          tabindex="0"
          onclick={() => openArtifactViewer(artifact)}
          onkeydown={(e) => e.key === 'Enter' && openArtifactViewer(artifact)}
        >
          <button
            class="artifact-select-btn"
            class:selected={selectedArtifactIds.has(artifact.id)}
            onclick={(e) => {
              e.stopPropagation();
              toggleArtifactSelection(artifact.id);
            }}
            title="Include in next chat context"
          >
            {#if selectedArtifactIds.has(artifact.id)}
              <CheckCircle2 size={14} />
            {:else}
              <Circle size={14} />
            {/if}
          </button>
          <FileText size={12} />
          <span class="artifact-title">{artifact.title}</span>
          {#if confirmingDeleteId === artifact.id}
            <div class="delete-confirm">
              <button
                class="delete-confirm-btn"
                onclick={(e) => {
                  e.stopPropagation();
                  deleteArtifact(artifact.id);
                  confirmingDeleteId = null;
                }}
                title="Confirm delete"
              >
                Delete
              </button>
              <button
                class="delete-cancel-btn"
                onclick={(e) => {
                  e.stopPropagation();
                  confirmingDeleteId = null;
                }}
                title="Cancel"
              >
                Cancel
              </button>
            </div>
          {:else}
            <button
              class="artifact-delete"
              onclick={(e) => {
                e.stopPropagation();
                confirmingDeleteId = artifact.id;
              }}
              title="Delete artifact"
            >
              <X size={12} />
            </button>
          {/if}
        </div>
      </div>
    {/each}

    <!-- Comments context item -->
    {#if totalCommentsCount > 0}
      <div class="artifact-item comments-context-item">
        <div class="artifact-header" role="button" tabindex="0">
          <button
            class="artifact-select-btn"
            class:selected={includeCommentsInContext}
            onclick={toggleCommentsInContext}
            title="Include all comments in next chat context"
          >
            {#if includeCommentsInContext}
              <CheckCircle2 size={14} />
            {:else}
              <Circle size={14} />
            {/if}
          </button>
          <MessageSquare size={12} />
          <span class="artifact-title">All comments ({totalCommentsCount})</span>
        </div>
      </div>
    {/if}
  </div>

  <!-- Bottom area: current response + input (anchored at bottom) -->
  <div class="agent-bottom">
    <!-- Live streaming view (shown during active streaming) -->
    {#if isStreaming && liveSession}
      <div class="agent-response">
        <div class="agent-response-header" class:disabled={true}>
          <span class="response-chevron">
            <Loader2 size={12} class="spinning" />
          </span>
          <Bot size={12} />
          <span class="response-label">Working on it...</span>
        </div>
        <div class="agent-response-content">
          <LiveSessionView session={liveSession} />
        </div>
      </div>
      <!-- Static response view (shown when not streaming and have response) -->
    {:else if agentState.loading || agentState.response}
      <div class="agent-response">
        <div
          class="agent-response-header"
          role="button"
          tabindex="0"
          onclick={() => !agentState.loading && (responseExpanded = !responseExpanded)}
          onkeydown={(e) =>
            e.key === 'Enter' && !agentState.loading && (responseExpanded = !responseExpanded)}
          class:disabled={agentState.loading}
        >
          <span class="response-chevron">
            {#if agentState.loading}
              <Loader2 size={12} class="spinning" />
            {:else if responseExpanded}
              <ChevronDown size={12} />
            {:else}
              <ChevronRight size={12} />
            {/if}
          </span>
          <Bot size={12} />
          <span class="response-label">
            {#if agentState.loading}
              Working on it...
            {:else}
              Response
            {/if}
          </span>
          {#if !agentState.loading && agentState.response}
            <div class="response-actions">
              <button
                class="save-btn"
                onclick={(e) => {
                  e.stopPropagation();
                  saveAsArtifact();
                }}
                title="Save as artifact"
              >
                <Save size={12} />
                <span>Save</span>
              </button>
              <button
                class="discard-btn"
                onclick={(e) => {
                  e.stopPropagation();
                  discardResponse();
                }}
                title="Discard and end session"
              >
                <Trash2 size={12} />
                <span>Discard</span>
              </button>
            </div>
          {/if}
        </div>
        {#if responseExpanded && !agentState.loading}
          <div class="agent-response-content">
            <div class="markdown-content">
              {@html renderedResponse}
            </div>
          </div>
        {/if}
      </div>
    {/if}

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

<!-- Artifact viewer modal -->
{#if viewingArtifact}
  <div
    class="artifact-modal-backdrop"
    role="button"
    tabindex="0"
    onclick={closeArtifactViewer}
    onkeydown={handleModalKeydown}
  >
    <div
      class="artifact-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="artifact-modal-title"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <div class="artifact-modal-header">
        <h2 id="artifact-modal-title" class="artifact-modal-title">{viewingArtifact.title}</h2>
        <button class="artifact-modal-close" onclick={closeArtifactViewer} title="Close">
          <X size={16} />
        </button>
      </div>
      <div class="artifact-modal-content markdown-content">
        {@html renderArtifactContent(viewingArtifact.content)}
      </div>
    </div>
  </div>
{/if}

<style>
  .agent-section {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    padding: 0 12px;
  }

  .agent-top {
    flex-shrink: 0;
    padding-top: 8px;
  }

  .agent-bottom {
    flex-shrink: 0;
    padding: 8px 0 0;
    margin-top: auto;
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

  .agent-send-btn :global(.spinning),
  .agent-response-header :global(.spinning) {
    animation: spin 1s linear infinite;
    transform-origin: center;
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

  /* Artifact items */
  .artifact-item {
    margin-bottom: 4px;
    background: var(--bg-primary);
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    overflow: hidden;
  }

  .artifact-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 10px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .artifact-header:hover {
    background-color: var(--bg-hover);
  }

  .artifact-select-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    margin: -4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    flex-shrink: 0;
    transition: color 0.1s;
  }

  .artifact-select-btn:hover {
    color: var(--text-muted);
  }

  .artifact-select-btn.selected {
    color: var(--text-accent);
  }

  .artifact-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Comments context item (not clickable to view) */
  .comments-context-item .artifact-header {
    cursor: default;
  }

  .comments-context-item .artifact-header:hover {
    background-color: var(--bg-primary);
  }

  .artifact-delete {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      background-color 0.1s,
      color 0.1s;
  }

  .artifact-header:hover .artifact-delete {
    opacity: 1;
  }

  .artifact-delete:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Delete confirmation inline */
  .delete-confirm {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .delete-confirm-btn {
    padding: 2px 8px;
    background: var(--ui-danger);
    border: none;
    border-radius: 3px;
    color: white;
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .delete-confirm-btn:hover {
    background: var(--ui-danger-hover, #c53030);
  }

  .delete-cancel-btn {
    padding: 2px 8px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .delete-cancel-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Agent response */
  .agent-response {
    margin-bottom: 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    overflow: hidden;
  }

  .agent-response-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-hover);
    border: none;
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .agent-response-header.disabled {
    cursor: default;
  }

  .agent-response-header:not(.disabled):hover {
    background-color: var(--bg-primary);
  }

  .response-chevron {
    display: flex;
    align-items: center;
  }

  .response-label {
    flex: 1;
  }

  .response-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .save-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      border-color 0.1s,
      color 0.1s;
  }

  .save-btn:hover {
    background-color: var(--bg-hover);
    border-color: var(--text-accent);
    color: var(--text-accent);
  }

  .discard-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      border-color 0.1s,
      color 0.1s;
  }

  .discard-btn:hover {
    background-color: var(--bg-hover);
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .agent-response-content {
    padding: 10px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    word-break: break-word;
    max-height: 200px;
    overflow-y: auto;
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

  /* Artifact modal */
  .artifact-modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 40px;
  }

  .artifact-modal {
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 12px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
    max-width: 700px;
    width: 100%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .artifact-modal-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .artifact-modal-title {
    flex: 1;
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .artifact-modal-close {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .artifact-modal-close:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .artifact-modal-content {
    flex: 1;
    padding: 20px;
    overflow-y: auto;
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.6;
  }
</style>
