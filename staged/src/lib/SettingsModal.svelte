<!--
  SettingsModal.svelte - User preferences and settings

  Sections:
  1. Layout - Sidebar position toggle
  2. Keyboard - Shortcut customization
-->
<script lang="ts">
  import {
    X,
    Settings,
    PanelLeft,
    PanelRight,
    Keyboard,
    RotateCcw,
    FlaskConical,
    Bot,
    ExternalLink,
    RefreshCw,
  } from 'lucide-svelte';
  import {
    preferences,
    toggleSidebarPosition,
    saveCustomKeyboardBinding,
    removeCustomKeyboardBinding,
    resetAllKeyboardBindings,
    DEFAULT_FEATURES,
    setFeatureFlag,
    resetFeatureFlags,
    setAiAgent,
    type SidebarPosition,
  } from './stores/preferences.svelte';
  import { agentGlobalState } from './stores/agent.svelte';
  import { discoverAcpProviders } from './services/ai';
  import { openUrl } from './services/window';
  import {
    getAllShortcuts,
    formatShortcutKeys,
    isMac,
    updateBinding,
    resetBinding,
    hasConflict,
    isCustomized,
    type Shortcut,
    type Modifiers,
  } from './services/keyboard';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  // Active tab
  type Tab = 'layout' | 'keyboard' | 'features';
  let activeTab = $state<Tab>('layout');

  // Feature flag metadata for display
  const featureMeta: Record<string, { label: string; description: string }> = {};

  // Get list of all feature flags with their current state
  function getFeatureFlags(): Array<{
    id: string;
    label: string;
    description: string;
    enabled: boolean;
  }> {
    const flags: Array<{ id: string; label: string; description: string; enabled: boolean }> = [];

    // Only include flags defined in DEFAULT_FEATURES (ignore legacy stored flags)
    for (const [id, defaultValue] of Object.entries(DEFAULT_FEATURES)) {
      const meta = featureMeta[id] || { label: id, description: '' };
      flags.push({
        id,
        label: meta.label,
        description: meta.description,
        enabled: preferences.features[id] ?? defaultValue,
      });
    }

    return flags;
  }

  let featureFlags = $derived(getFeatureFlags());

  // Known agents with metadata
  const KNOWN_AGENTS = [
    {
      id: 'goose',
      label: 'Goose',
      installUrl: 'https://github.com/block/goose',
    },
    {
      id: 'claude',
      label: 'Claude Code',
      installUrl: 'https://github.com/zed-industries/claude-code-acp#installation',
    },
    {
      id: 'codex',
      label: 'Codex',
      installUrl: 'https://github.com/zed-industries/codex-acp#installation',
    },
  ];

  // AI Agent state
  let refreshingProviders = $state(false);

  async function refreshProviders() {
    refreshingProviders = true;

    // Force UI update and paint before async work
    await new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)));

    // Ensure spinner shows for at least 400ms for better UX
    const startTime = Date.now();

    try {
      const newProviders = await discoverAcpProviders();
      // Only update if providers actually changed (avoid UI flicker)
      const currentIds = agentGlobalState.availableProviders
        .map((p) => p.id)
        .sort()
        .join(',');
      const newIds = newProviders
        .map((p) => p.id)
        .sort()
        .join(',');
      if (currentIds !== newIds) {
        agentGlobalState.availableProviders = newProviders;
      }
      agentGlobalState.providersLoaded = true;
    } catch (e) {
      console.error('Failed to refresh providers:', e);
    } finally {
      // Wait for minimum display time before hiding spinner
      const elapsed = Date.now() - startTime;
      const minDisplayTime = 400;
      if (elapsed < minDisplayTime) {
        await new Promise((resolve) => setTimeout(resolve, minDisplayTime - elapsed));
      }
      refreshingProviders = false;
    }
  }

  function isAgentAvailable(agentId: string): boolean {
    return agentGlobalState.availableProviders.some((p) => p.id === agentId);
  }

  function getAgentLabel(agentId: string): string {
    return KNOWN_AGENTS.find((a) => a.id === agentId)?.label ?? agentId;
  }

  function openInstallUrl(url: string) {
    openUrl(url);
  }

  // Keyboard rebinding state
  let editingShortcutId = $state<string | null>(null);
  let capturedKeys = $state<string[]>([]);
  let capturedModifiers = $state<Modifiers>({});
  let conflictId = $state<string | null>(null);

  // Version counter to force reactivity when shortcuts are reset
  let shortcutVersion = $state(0);

  // Category display names and order
  const categoryInfo: Record<string, { title: string; order: number }> = {
    navigation: { title: 'Navigation', order: 1 },
    view: { title: 'View', order: 2 },
    comments: { title: 'Comments', order: 3 },
    files: { title: 'Files', order: 4 },
  };

  interface ShortcutGroup {
    title: string;
    order: number;
    shortcuts: Shortcut[];
  }

  function getGroupedShortcuts(_version: number): ShortcutGroup[] {
    const allShortcuts = getAllShortcuts();
    const byCategory = new Map<string, Shortcut[]>();

    for (const shortcut of allShortcuts) {
      const copy = { ...shortcut, keys: [...shortcut.keys] };
      const list = byCategory.get(shortcut.category) || [];
      list.push(copy);
      byCategory.set(shortcut.category, list);
    }

    const groups: ShortcutGroup[] = [];

    for (const [category, shortcuts] of byCategory) {
      const info = categoryInfo[category] || { title: category, order: 99 };
      groups.push({
        title: info.title,
        order: info.order,
        shortcuts,
      });
    }

    return groups.sort((a, b) => a.order - b.order);
  }

  let shortcutGroups = $derived(getGroupedShortcuts(shortcutVersion));

  function formatKeys(shortcut: Shortcut): string {
    const formatted = formatShortcutKeys(shortcut);
    return formatted.map((f) => f.modifiers.join('') + f.key).join(' / ');
  }

  function startEditing(id: string) {
    editingShortcutId = id;
    capturedKeys = [];
    capturedModifiers = {};
    conflictId = null;
  }

  function cancelEditing() {
    editingShortcutId = null;
    capturedKeys = [];
    capturedModifiers = {};
    conflictId = null;
  }

  function confirmBinding() {
    if (!editingShortcutId || capturedKeys.length === 0) return;

    // Check for conflicts
    const conflict = hasConflict(capturedKeys, capturedModifiers, editingShortcutId);
    if (conflict) {
      conflictId = conflict;
      return;
    }

    // Update the binding
    updateBinding(editingShortcutId, capturedKeys, capturedModifiers);
    saveCustomKeyboardBinding(editingShortcutId, {
      keys: capturedKeys,
      modifiers: capturedModifiers,
    });

    shortcutVersion++;
    cancelEditing();
  }

  function resetShortcut(id: string) {
    resetBinding(id);
    removeCustomKeyboardBinding(id);
    shortcutVersion++;
  }

  function resetAllShortcuts() {
    const allShortcuts = getAllShortcuts();
    for (const shortcut of allShortcuts) {
      resetBinding(shortcut.id);
    }
    resetAllKeyboardBindings();
    shortcutVersion++;
  }

  function handleKeyCapture(event: KeyboardEvent) {
    if (!editingShortcutId) return;

    event.preventDefault();
    event.stopPropagation();

    // Ignore modifier-only keypresses
    if (['Control', 'Meta', 'Shift', 'Alt'].includes(event.key)) {
      return;
    }

    // Escape cancels
    if (event.key === 'Escape') {
      cancelEditing();
      return;
    }

    // Enter confirms
    if (event.key === 'Enter') {
      confirmBinding();
      return;
    }

    capturedKeys = [event.key];
    capturedModifiers = {
      ctrl: event.ctrlKey,
      meta: event.metaKey,
      shift: event.shiftKey,
      alt: event.altKey,
    };

    // Clear conflict when new key is captured
    conflictId = null;
  }

  function formatCapturedKey(): string {
    if (capturedKeys.length === 0) return '';

    const parts: string[] = [];
    if (isMac()) {
      if (capturedModifiers.ctrl) parts.push('\u2303');
      if (capturedModifiers.alt) parts.push('\u2325');
      if (capturedModifiers.shift) parts.push('\u21e7');
      if (capturedModifiers.meta) parts.push('\u2318');
    } else {
      if (capturedModifiers.ctrl) parts.push('Ctrl+');
      if (capturedModifiers.meta) parts.push('Ctrl+');
      if (capturedModifiers.alt) parts.push('Alt+');
      if (capturedModifiers.shift) parts.push('Shift+');
    }

    let key = capturedKeys[0];
    if (key === 'ArrowDown') key = '\u2193';
    else if (key === 'ArrowUp') key = '\u2191';
    else if (key === 'ArrowLeft') key = '\u2190';
    else if (key === 'ArrowRight') key = '\u2192';
    else key = key.toUpperCase();

    parts.push(key);
    return parts.join('');
  }

  function getConflictDescription(): string {
    if (!conflictId) return '';
    const shortcut = getAllShortcuts().find((s) => s.id === conflictId);
    return shortcut ? shortcut.description : conflictId;
  }

  function handleKeydown(event: KeyboardEvent) {
    // Don't close on Escape if editing
    if (editingShortcutId) return;

    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      if (editingShortcutId) {
        cancelEditing();
      } else {
        onClose();
      }
    }
  }
</script>

<svelte:window onkeydown={editingShortcutId ? handleKeyCapture : handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => !editingShortcutId && e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <header class="modal-header">
      <h2>
        <Settings size={16} />
        Settings
      </h2>
      <button class="close-btn" onclick={onClose}>
        <X size={16} />
      </button>
    </header>

    <div class="modal-body">
      <!-- Tabs -->
      <div class="tabs">
        <button
          class="tab"
          class:active={activeTab === 'layout'}
          onclick={() => (activeTab = 'layout')}
        >
          <Settings size={14} />
          General
        </button>
        <button
          class="tab"
          class:active={activeTab === 'keyboard'}
          onclick={() => (activeTab = 'keyboard')}
        >
          <Keyboard size={14} />
          Keyboard
        </button>
        {#if featureFlags.length > 0}
          <button
            class="tab"
            class:active={activeTab === 'features'}
            onclick={() => (activeTab = 'features')}
          >
            <FlaskConical size={14} />
            Features
          </button>
        {/if}
      </div>

      <!-- Layout Tab -->
      {#if activeTab === 'layout'}
        <div class="tab-content">
          <div class="setting-row">
            <div class="setting-info">
              <div class="setting-label">Sidebar Position</div>
              <div class="setting-description">Choose which side the file list appears on</div>
            </div>
            <div class="toggle-group">
              <button
                class="toggle-btn"
                class:active={preferences.sidebarPosition === 'left'}
                onclick={() => preferences.sidebarPosition !== 'left' && toggleSidebarPosition()}
              >
                <PanelLeft size={14} />
                Left
              </button>
              <button
                class="toggle-btn"
                class:active={preferences.sidebarPosition === 'right'}
                onclick={() => preferences.sidebarPosition !== 'right' && toggleSidebarPosition()}
              >
                <PanelRight size={14} />
                Right
              </button>
            </div>
          </div>

          <div class="section-divider"></div>

          <div class="setting-section">
            <div class="section-header">
              <div class="section-title">
                <Bot size={14} />
                AI Agent
              </div>
              <button
                class="refresh-btn-small"
                onclick={refreshProviders}
                disabled={refreshingProviders}
                title="Check for newly installed agents"
              >
                <span class="spin-container" class:spinning={refreshingProviders}
                  ><RefreshCw size={12} /></span
                >
              </button>
            </div>
            <div class="agents-list-settings">
              {#each KNOWN_AGENTS as agent}
                {@const available = isAgentAvailable(agent.id)}
                <div class="agent-row" class:unavailable={!available}>
                  <button
                    class="agent-select-btn"
                    class:selected={preferences.aiAgent === agent.id}
                    disabled={!available}
                    onclick={() => available && setAiAgent(agent.id)}
                  >
                    <span class="agent-name">{agent.label}</span>
                    {#if !available}
                      <span class="agent-status">Not installed</span>
                    {:else if preferences.aiAgent === agent.id}
                      <span class="agent-status selected">Selected</span>
                    {/if}
                  </button>
                  {#if !available}
                    <button
                      class="install-link"
                      onclick={() => openInstallUrl(agent.installUrl)}
                      title="Install {agent.label}"
                    >
                      <ExternalLink size={12} />
                    </button>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        </div>
      {/if}

      <!-- Keyboard Tab -->
      {#if activeTab === 'keyboard'}
        <div class="tab-content keyboard-tab">
          <div class="keyboard-header">
            <span class="keyboard-hint">Click a shortcut to rebind it</span>
            <button class="reset-all-btn" onclick={resetAllShortcuts}>
              <RotateCcw size={12} />
              Reset All
            </button>
          </div>

          <div class="shortcuts-list">
            {#each shortcutGroups as group}
              <div class="shortcut-group">
                <div class="group-title">{group.title}</div>
                {#each group.shortcuts as shortcut}
                  <div class="shortcut-row" class:editing={editingShortcutId === shortcut.id}>
                    <span class="shortcut-description">{shortcut.description}</span>
                    {#if editingShortcutId === shortcut.id}
                      <div class="capture-area">
                        {#if capturedKeys.length > 0}
                          <span class="captured-key" class:conflict={conflictId}>
                            {formatCapturedKey()}
                          </span>
                          {#if conflictId}
                            <span class="conflict-msg"
                              >Conflicts with "{getConflictDescription()}"</span
                            >
                          {/if}
                        {:else}
                          <span class="capture-prompt">Press a key...</span>
                        {/if}
                        <div class="capture-actions">
                          <button class="action-btn" onclick={cancelEditing}>Cancel</button>
                          <button
                            class="action-btn primary"
                            onclick={confirmBinding}
                            disabled={capturedKeys.length === 0 || !!conflictId}
                          >
                            Save
                          </button>
                        </div>
                      </div>
                    {:else}
                      <div class="shortcut-actions">
                        <button class="key-btn" onclick={() => startEditing(shortcut.id)}>
                          {formatKeys(shortcut)}
                        </button>
                        {#if isCustomized(shortcut.id)}
                          <button
                            class="reset-btn"
                            onclick={() => resetShortcut(shortcut.id)}
                            title="Reset to default"
                          >
                            <RotateCcw size={12} />
                          </button>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Features Tab -->
      {#if activeTab === 'features'}
        <div class="tab-content features-tab">
          {#if featureFlags.length === 0}
            <div class="empty-state">
              <FlaskConical size={24} />
              <p>No feature flags available yet.</p>
              <span class="empty-hint"
                >Feature flags will appear here as they're added to the app.</span
              >
            </div>
          {:else}
            <div class="features-header">
              <span class="features-hint">Enable or disable experimental features</span>
              <button class="reset-all-btn" onclick={resetFeatureFlags}>
                <RotateCcw size={12} />
                Reset All
              </button>
            </div>

            <div class="features-list">
              {#each featureFlags as flag}
                <div class="feature-row">
                  <div class="setting-info">
                    <div class="setting-label">{flag.label}</div>
                    {#if flag.description}
                      <div class="setting-description">{flag.description}</div>
                    {/if}
                  </div>
                  <button
                    class="toggle-switch"
                    class:active={flag.enabled}
                    onclick={() => setFeatureFlag(flag.id, !flag.enabled)}
                    role="switch"
                    aria-checked={flag.enabled}
                    aria-label="Toggle {flag.label}"
                  >
                    <span class="toggle-knob"></span>
                  </button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 520px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .modal-header h2 :global(svg) {
    color: var(--text-muted);
  }

  .close-btn {
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
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .modal-body {
    padding: 0;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
  }

  /* Tabs */
  .tabs {
    display: flex;
    border-bottom: 1px solid var(--border-subtle);
    padding: 0 20px;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 12px 16px;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
    margin-bottom: -1px;
  }

  .tab:hover {
    color: var(--text-primary);
  }

  .tab.active {
    color: var(--text-primary);
    border-bottom-color: var(--ui-accent);
  }

  .tab :global(svg) {
    opacity: 0.7;
  }

  .tab.active :global(svg) {
    opacity: 1;
  }

  /* Tab Content */
  .tab-content {
    padding: 20px;
  }

  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
  }

  .setting-info {
    flex: 1;
  }

  .setting-label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .setting-description {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .toggle-group {
    display: flex;
    gap: 4px;
    background: var(--bg-primary);
    border-radius: 6px;
    padding: 4px;
  }

  .toggle-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .toggle-btn:hover {
    color: var(--text-primary);
  }

  .toggle-btn.active {
    background: var(--bg-chrome);
    color: var(--text-primary);
    box-shadow: var(--shadow-subtle);
  }

  /* Section Divider */
  .section-divider {
    height: 1px;
    background: var(--border-subtle);
    margin: 20px 0;
  }

  /* AI Agent Section */
  .setting-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .section-title :global(svg) {
    color: var(--text-muted);
  }

  .refresh-btn-small {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    border-radius: 4px;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .refresh-btn-small:not(:disabled):hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .refresh-btn-small:disabled {
    cursor: not-allowed;
  }

  .agents-list-settings {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .agent-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .agent-select-btn {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 2px solid transparent;
    border-radius: 6px;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background-color 0.15s;
  }

  .agent-select-btn:not(:disabled):hover {
    background: var(--bg-hover);
  }

  .agent-select-btn.selected {
    border-color: var(--ui-accent);
  }

  .agent-select-btn:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .agent-name {
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .agent-status {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .agent-status.selected {
    color: var(--ui-accent);
  }

  .install-link {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 8px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .install-link:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin-container {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .spin-container.spinning {
    animation: spin 1s linear infinite;
  }

  /* Keyboard Tab */
  .keyboard-tab {
    padding: 12px 20px 20px;
  }

  .keyboard-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }

  .keyboard-hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .reset-all-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .reset-all-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .shortcuts-list {
    max-height: 400px;
    overflow-y: auto;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
  }

  .shortcut-group {
    padding: 8px 12px;
  }

  .shortcut-group:not(:last-child) {
    border-bottom: 1px solid var(--border-subtle);
  }

  .group-title {
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
  }

  .shortcut-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 0;
    gap: 12px;
  }

  .shortcut-row.editing {
    background: var(--bg-hover);
    margin: 0 -12px;
    padding: 8px 12px;
    border-radius: 4px;
  }

  .shortcut-description {
    font-size: var(--size-xs);
    color: var(--text-primary);
    white-space: nowrap;
  }

  .shortcut-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .key-btn {
    padding: 4px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .key-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .reset-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    transition: color 0.1s;
  }

  .reset-btn:hover {
    color: var(--text-primary);
  }

  .capture-area {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    justify-content: flex-end;
  }

  .capture-prompt {
    font-size: var(--size-xs);
    color: var(--text-faint);
    font-style: italic;
  }

  .captured-key {
    padding: 4px 8px;
    background: var(--ui-accent);
    color: var(--bg-primary);
    border-radius: 4px;
    font-size: var(--size-xs);
    font-weight: 500;
  }

  .captured-key.conflict {
    background: var(--ui-danger);
  }

  .conflict-msg {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--ui-danger);
  }

  .capture-actions {
    display: flex;
    gap: 4px;
  }

  .action-btn {
    padding: 4px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .action-btn:hover:not(:disabled) {
    color: var(--text-primary);
  }

  .action-btn.primary {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: var(--bg-primary);
  }

  .action-btn.primary:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Features Tab */
  .features-tab {
    padding: 12px 20px 20px;
  }

  .features-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }

  .features-hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .features-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .feature-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 12px;
    background: var(--bg-primary);
    border-radius: 8px;
  }

  .toggle-switch {
    position: relative;
    width: 44px;
    height: 24px;
    background: var(--border-muted);
    border: none;
    border-radius: 12px;
    cursor: pointer;
    transition: background-color 0.2s;
    flex-shrink: 0;
  }

  .toggle-switch.active {
    background: var(--ui-accent);
  }

  .toggle-knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 20px;
    height: 20px;
    background: white;
    border-radius: 50%;
    transition: transform 0.2s;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  }

  .toggle-switch.active .toggle-knob {
    transform: translateX(20px);
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 40px 20px;
    color: var(--text-muted);
    text-align: center;
  }

  .empty-state :global(svg) {
    color: var(--text-faint);
    margin-bottom: 12px;
  }

  .empty-state p {
    margin: 0 0 4px 0;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .empty-hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }
</style>
