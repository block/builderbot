<script lang="ts">
  import { onMount } from 'svelte';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import Info from '@lucide/svelte/icons/info';
  import Check from '@lucide/svelte/icons/check';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import * as Select from '$lib/components/ui/select';
  import * as Popover from '$lib/components/ui/popover';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectDiffTheme,
    setMode,
    setAutoReviewMode,
    setBranchPrefix,
    loadAllThemePreviewColors,
    isLightTheme,
    type AppMode,
    type AutoReviewMode,
    type ThemePreviewColors,
  } from './preferences.svelte';

  const autoReviewOptions: { value: AutoReviewMode; label: string }[] = [
    { value: 'never', label: 'Never' },
    { value: 'after-changes', label: 'After changes' },
  ];

  const modeOptions: { value: AppMode; label: string }[] = [
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
    { value: 'system', label: 'System' },
  ];

  type ThemeFilter = 'all' | 'light' | 'dark';

  const allThemes = $derived(getAvailableSyntaxThemes());

  let themeFilter = $state<ThemeFilter>('all');
  let previewColors = $state<Map<string, ThemePreviewColors>>(new Map());
  let dropdownOpen = $state(false);

  const themes = $derived(
    themeFilter === 'all'
      ? allThemes
      : allThemes.filter((t) =>
          themeFilter === 'light' ? isLightTheme(t.name) : !isLightTheme(t.name)
        )
  );

  const activeColors = $derived(previewColors.get(preferences.diffTheme));

  onMount(() => {
    loadAllThemePreviewColors().then((colors) => {
      previewColors = colors;
    });
  });

  function handleThemeSelect(name: string) {
    selectDiffTheme(name);
    dropdownOpen = false;
  }

  const branchPrefixExample = $derived.by(() => {
    const prefix = preferences.branchPrefix.trim();
    if (!prefix) return 'my-project';
    return prefix.endsWith('/') ? `${prefix}my-project` : `${prefix}/my-project`;
  });
</script>

<div class="general-settings-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Settings2 size={16} />
        General
      </h2>
      <p>Appearance and app behaviour.</p>
    </div>
  </div>

  <div class="panel-body">
    <div class="field">
      <span class="field-label">Appearance</span>
      <ToggleGroup.Root
        type="single"
        variant="outline"
        value={preferences.mode}
        onValueChange={(v) => v && setMode(v as AppMode)}
        class="w-fit"
      >
        {#each modeOptions as opt (opt.value)}
          <ToggleGroup.Item value={opt.value} aria-label={opt.label}>
            {opt.label}
          </ToggleGroup.Item>
        {/each}
      </ToggleGroup.Root>
      <p class="field-description">
        <Info size={12} />
        The app's light or dark mode. System follows your operating system.
      </p>
    </div>

    <div class="field">
      <span class="field-label">Diff theme</span>
      <Popover.Root bind:open={dropdownOpen}>
        <Popover.Trigger class="theme-dropdown-trigger">
          <span class="trigger-swatch" style:background={activeColors?.bg ?? 'var(--bg-primary)'}>
            <span style:color={activeColors?.fg ?? 'var(--text-primary)'}
              >{preferences.diffTheme}</span
            >
          </span>
          <ChevronDown size={14} />
        </Popover.Trigger>
        <Popover.Content
          align="start"
          sideOffset={4}
          class="theme-dropdown-panel w-[var(--bits-popover-anchor-width)] ring-0"
        >
          <div class="theme-filters">
            {#each ['all', 'light', 'dark'] as filter (filter)}
              <button
                class="theme-filter-btn"
                class:active={themeFilter === filter}
                onclick={() => (themeFilter = filter as ThemeFilter)}
              >
                {filter.charAt(0).toUpperCase() + filter.slice(1)}
              </button>
            {/each}
          </div>
          {#each themes as theme (theme.name)}
            {@const colors = previewColors.get(theme.name)}
            {@const isActive = preferences.diffTheme === theme.name}
            <button
              class="theme-swatch"
              class:active={isActive}
              style:background={colors?.bg ?? 'var(--bg-primary)'}
              style:border-color={isActive
                ? (colors?.comment ?? 'var(--border-emphasis)')
                : 'transparent'}
              onclick={() => handleThemeSelect(theme.name)}
            >
              <span class="swatch-name" style:color={colors?.fg ?? 'var(--text-primary)'}
                >{theme.name}</span
              >
              {#if isActive}
                <span class="swatch-check" style:color={colors?.comment ?? 'var(--text-muted)'}>
                  <Check size={14} />
                </span>
              {/if}
            </button>
          {/each}
        </Popover.Content>
      </Popover.Root>
      <p class="field-description">
        <Info size={12} />
        Syntax highlighting and colors for the diff viewer only.
      </p>
    </div>

    <div class="field">
      <Label for="auto-review-select" class="text-foreground text-sm font-semibold"
        >Auto start code reviews</Label
      >
      <Select.Root
        type="single"
        value={preferences.autoReviewMode}
        onValueChange={(v) => setAutoReviewMode(v as AutoReviewMode)}
      >
        <Select.Trigger id="auto-review-select" class="w-full max-w-[320px]">
          {autoReviewOptions.find((o) => o.value === preferences.autoReviewMode)?.label ?? ''}
        </Select.Trigger>
        <Select.Content>
          {#each autoReviewOptions as opt (opt.value)}
            <Select.Item value={opt.value} label={opt.label}>{opt.label}</Select.Item>
          {/each}
        </Select.Content>
      </Select.Root>
      <p class="field-description">
        <Info size={12} />
        {#if preferences.autoReviewMode === 'after-changes'}
          A code review will automatically start after each commit session completes.
        {:else}
          Code reviews will only start when you manually request them.
        {/if}
      </p>
    </div>

    <div class="field">
      <Label for="branch-prefix-input" class="text-foreground text-sm font-semibold"
        >Branch prefix</Label
      >
      <Input
        id="branch-prefix-input"
        type="text"
        placeholder="e.g. alice"
        class="max-w-[320px]"
        value={preferences.branchPrefix}
        oninput={(e) => setBranchPrefix(e.currentTarget.value)}
      />
      <p class="field-description">
        <Info size={12} />
        {#if preferences.branchPrefix.trim()}
          Branch names generated from project names will look like {branchPrefixExample}.
        {:else}
          This prefix will be added to branch names along with a slash separator when a repo is
          added without choosing a branch.
        {/if}
      </p>
    </div>
  </div>
</div>

<style>
  .general-settings-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--bg-chrome);
  }

  .panel-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .intro-copy {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .panel-intro h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-base);
    font-weight: 600;
  }

  .panel-intro p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-description {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    display: flex;
    align-items: baseline;
    gap: 4px;
  }

  .field-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  :global(.theme-dropdown-trigger) {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    max-width: 320px;
    padding: 0;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    overflow: hidden;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  :global(.theme-dropdown-trigger:hover) {
    border-color: var(--border-emphasis);
  }

  .trigger-swatch {
    flex: 1;
    padding: 6px 10px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border-radius: 4px 0 0 4px;
  }

  :global(.theme-dropdown-trigger svg) {
    flex-shrink: 0;
    margin-right: 8px;
    color: var(--text-muted);
  }

  :global(.theme-dropdown-panel) {
    max-height: 320px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    box-shadow: var(--shadow-elevated);
  }

  .theme-filters {
    display: flex;
    gap: 2px;
    padding: 0 0 4px;
    border-bottom: 1px solid var(--border-subtle);
    margin-bottom: 2px;
  }

  .theme-filter-btn {
    flex: 1;
    padding: 4px 8px;
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

  .theme-filter-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .theme-filter-btn.active {
    background: var(--bg-active);
    color: var(--text-primary);
    font-weight: 600;
  }

  .theme-swatch {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border: 1.5px solid transparent;
    border-radius: 4px;
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.1s;
  }

  .theme-swatch:hover {
    filter: brightness(1.08);
  }

  .theme-swatch.active {
    border-style: solid;
  }

  .swatch-name {
    font-size: var(--size-xs);
    font-weight: 500;
    white-space: nowrap;
  }

  .swatch-check {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    margin-left: auto;
  }
</style>
