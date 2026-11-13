<!--
  RepoSearchInput.svelte - Inline repo search with dropdown results

  Self-contained text input that fetches recent/user repos on mount
  and shows a dropdown when focused. The dropdown is portaled and fixed
  (positioned from the field's rect, as HashtagInput's # dropdown is from
  the caret): in a full-screen dialog the form is the dialog's scroller,
  which would otherwise clip it to the form's overflow.
  Used in NewProjectForm and AddRepoModal for the repository field.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import Search from '@lucide/svelte/icons/search';
  import Lock from '@lucide/svelte/icons/lock';
  import Globe from '@lucide/svelte/icons/globe';
  import Clock from '@lucide/svelte/icons/clock';
  import Plus from '@lucide/svelte/icons/plus';
  import Command from '@lucide/svelte/icons/command';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { Input } from '$lib/components/ui/input';
  import * as commands from '../../api/commands';
  import type { GitHubRepo, RecentRepo } from '../../types';
  import { parseGitHubUrl, type RepoSelection } from '../../shared/githubUrl';
  import { matchesRepoSearch } from '../../shared/repoSearch';
  import { matchesAllTerms, queryTerms } from '../../shared/textSearch';
  import { viewport } from '../../shared/viewport.svelte';
  import { autofocusOnOpen } from '../../shared/focusAfterDialogEnter';
  import { portal } from '../../shared/portal';
  import { visibleViewportRect } from '../../shared/visibleViewportRect';
  import { placeHashtagDropdown } from '../sessions/hashtagDropdownPlacement';
  import { activeDescendantId, listboxOptionId } from '../../shared/listboxIds';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import * as badge from '../../shared/badgeColors';

  export type { RepoSelection };

  interface Props {
    onSelect: (selection: RepoSelection) => void;
    disabled?: boolean;
    excludeRepos?: Set<string>;
    autofocus?: boolean;
  }

  let { onSelect, disabled = false, excludeRepos = new Set(), autofocus = false }: Props = $props();

  let recentRepos = $state<RecentRepo[]>([]);
  let repos = $state<GitHubRepo[]>([]);
  let searchResults = $state<GitHubRepo[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let query = $state('');
  let inputEl: HTMLInputElement | null = $state(null);
  let wrapperEl: HTMLDivElement | null = $state(null);
  let searchBoxEl: HTMLDivElement | null = $state(null);
  let dropdownEl: HTMLDivElement | null = $state(null);
  let dropdownStyle = $state('');
  let dropdownAbove = $state(false);
  /** The highlighted `.repo-item`, in DOM order; -1 for none. */
  let selectedIndex = $state(-1);
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let isSearching = $state(false);
  let directFetchRepo = $state<GitHubRepo | null>(null);
  let dropdownOpen = $state(false);
  const uid = $props.id();

  /** The dropdown's tallest, as `.repo-dropdown`'s max-height was. */
  const DROPDOWN_MAX_HEIGHT_PX = 280;

  // Place the dropdown from the field's rect on whichever side of it has
  // room in the visible viewport: with the iOS keyboard up that is the part
  // of the screen above the keyboard, and in a full-screen dialog the form
  // scrolls under the field. The same placement the # dropdown uses.
  function updateDropdownPosition() {
    if (!searchBoxEl) return;
    const rect = searchBoxEl.getBoundingClientRect();
    const placement = placeHashtagDropdown(
      { top: rect.top, bottom: rect.bottom, left: rect.left },
      visibleViewportRect(),
      { width: rect.width, maxHeight: DROPDOWN_MAX_HEIGHT_PX }
    );
    dropdownAbove = placement.side === 'above';
    dropdownStyle =
      `top:${placement.top}px; left:${placement.left}px;` +
      ` width:${placement.width}px; max-height:${placement.maxHeight}px;` +
      (placement.side === 'above' ? ' translate:0 -100%;' : '');
  }

  // While open, follow the field: the visual viewport (the keyboard coming
  // up, the dialog re-pinning to it), any scroller around the field (the
  // form in a full-screen dialog), the window, and the end of the dialog's
  // entry animation, during which the field's rect is still scaled. Each
  // is re-measured once more on the next frame, as the dialog moves on the
  // same events.
  $effect(() => {
    if (!dropdownOpen) return;
    let frame = 0;
    const update = () => {
      updateDropdownPosition();
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(updateDropdownPosition);
    };
    update();
    const vv = window.visualViewport;
    window.addEventListener('resize', update);
    document.addEventListener('scroll', update, true);
    document.addEventListener('animationend', update, true);
    document.addEventListener('transitionend', update, true);
    vv?.addEventListener('resize', update);
    vv?.addEventListener('scroll', update);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener('resize', update);
      document.removeEventListener('scroll', update, true);
      document.removeEventListener('animationend', update, true);
      document.removeEventListener('transitionend', update, true);
      vv?.removeEventListener('resize', update);
      vv?.removeEventListener('scroll', update);
    };
  });

  function openDropdown() {
    selectedIndex = -1;
    dropdownOpen = true;
  }

  function dropdownItems(): HTMLElement[] {
    return Array.from(dropdownEl?.querySelectorAll<HTMLElement>('.repo-item') ?? []);
  }

  function isOwnerRepoFormat(input: string): boolean {
    const trimmed = input.trim();
    return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(trimmed);
  }

  function parseQuery(input: string): { owner: string | null; term: string } {
    const trimmed = input.trim();
    const slashIndex = trimmed.indexOf('/');
    if (slashIndex > 0) {
      return {
        owner: trimmed.slice(0, slashIndex),
        term: trimmed.slice(slashIndex + 1),
      };
    }
    return { owner: null, term: trimmed };
  }

  let displayItems = $derived.by(() => {
    const seen = new Set<string>();
    const result: Array<{ type: 'recent' | 'repo'; data: RecentRepo | GitHubRepo }> = [];

    if (directFetchRepo) {
      result.push({ type: 'repo', data: directFetchRepo });
      seen.add(directFetchRepo.nameWithOwner);
    }

    for (const r of searchResults) {
      if (!seen.has(r.nameWithOwner)) {
        result.push({ type: 'repo', data: r });
        seen.add(r.nameWithOwner);
      }
    }

    const terms = queryTerms(query);
    const filtered =
      terms.length > 0
        ? repos.filter((r) => matchesAllTerms(terms, r.nameWithOwner, r.description))
        : repos;

    for (const r of filtered) {
      if (!seen.has(r.nameWithOwner)) {
        result.push({ type: 'repo', data: r });
        seen.add(r.nameWithOwner);
      }
    }

    return result;
  });

  let filteredRecentRepos = $derived.by(() => {
    const q = query.trim();
    const base = recentRepos.filter(
      (r) => !excludeRepos.has(`${r.githubRepo}\x00${r.subpath ?? ''}`)
    );
    return q ? base.filter((r) => matchesRepoSearch(r.githubRepo, r.subpath, q)) : base;
  });

  // ARIA combobox: the field keeps the focus and the dropdown is a listbox of
  // options that take none, so the field names the highlighted option through
  // aria-activedescendant. Options are numbered in DOM order (recent repos
  // first), the order selectedIndex counts in. The reference is dropped
  // whenever the dropdown is closed or nothing is highlighted.
  const listboxId = `${uid}-listbox`;
  const repoItemsShown = $derived(!loading && !error && !isSearching);
  const optionCount = $derived(
    filteredRecentRepos.length + (repoItemsShown ? displayItems.length : 0)
  );
  const activeOptionId = $derived(
    activeDescendantId(listboxId, { open: dropdownOpen, count: optionCount, selectedIndex })
  );

  const dark = $derived(darkMode.value);

  $effect(() => {
    if (recentRepos.length > 0) {
      repoBadgeStore.ensureForRepos(
        recentRepos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
      );
    }
  });

  function recentHue(r: RecentRepo): number {
    return repoBadgeStore.lookup(r.githubRepo, r.subpath)?.hue ?? 210;
  }

  onMount(async () => {
    if (autofocus) {
      // Defer so the click that opened our parent modal finishes bubbling
      // before we open the dropdown. Otherwise the trailing window-level
      // handleClickOutside (registered when this component mounted) sees a
      // target outside our wrapper and closes the dropdown we just opened.
      // On an iPhone or iPad the field waits for a tap (see autofocusOnOpen).
      setTimeout(() => autofocusOnOpen(inputEl), 0);
    }
    try {
      recentRepos = await commands.listRecentRepos(10);
      repos = await commands.listUserRepos(30);
    } catch (e) {
      error = typeof e === 'string' ? e : String(e);
    } finally {
      loading = false;
    }
  });

  onDestroy(() => {
    if (searchTimer) clearTimeout(searchTimer);
  });

  function handleSelect(selection: RepoSelection) {
    query = '';
    dropdownOpen = false;
    // Items take no focus (the dropdown's mousedown is cancelled, so the
    // field keeps it); let it go, so it doesn't jump into the
    // newly-revealed BranchPicker.
    inputEl?.blur();
    onSelect(selection);
  }

  async function handleInput() {
    openDropdown();
    const trimmed = query.trim();

    const parsed = parseGitHubUrl(trimmed);
    if (parsed) {
      handleSelect(parsed);
      // Blur so focus doesn't jump into the newly-revealed BranchPicker
      inputEl?.blur();
      return;
    }

    directFetchRepo = null;
    searchResults = [];

    if (searchTimer) clearTimeout(searchTimer);

    if (!trimmed) {
      isSearching = false;
      return;
    }

    isSearching = true;

    searchTimer = setTimeout(async () => {
      try {
        if (isOwnerRepoFormat(trimmed)) {
          const [owner, repo] = trimmed.split('/');
          try {
            const result = await commands.getGithubRepo(owner, repo);
            if (result) {
              directFetchRepo = result;
            }
          } catch {
            // Direct fetch failed, continue to search
          }
        }

        const { owner, term } = parseQuery(trimmed);

        if (term.length >= 1 || owner) {
          const searchQuery = term || (owner ? `org:${owner}` : '');
          if (searchQuery) {
            const results = await commands.searchGithubRepos(searchQuery, owner ?? undefined);
            searchResults = results;
          }
        }
      } catch {
        // Search failed — just use client-side filter
      }

      isSearching = false;
    }, 300);
  }

  function handleFocus() {
    openDropdown();
  }

  function isInside(node: Node | null): boolean {
    return !!node && (!!wrapperEl?.contains(node) || !!dropdownEl?.contains(node));
  }

  function handleFocusOut(e: FocusEvent) {
    if (isInside(e.relatedTarget as Node | null)) return;
    dropdownOpen = false;
  }

  function handleClickOutside(e: MouseEvent) {
    if (!isInside(e.target as Node)) dropdownOpen = false;
  }

  // The field keeps the focus throughout; the arrows move a highlight over
  // the items (as in HashtagInput) rather than the focus, which the
  // dialog's focus trap would pull back from the portaled dropdown.
  function handleKeydown(e: KeyboardEvent) {
    // A key the IME is handling (Enter committing a composition, arrows picking
    // a candidate, Escape cancelling it) is the IME's, not the dropdown's, as in
    // HashtagInput. Safari's committing Enter has isComposing false but
    // keyCode 229; without this it picked the highlighted repo mid-composition.
    if (!dropdownOpen || isImeKeydown(e)) return;

    if (e.key === 'Escape') {
      e.preventDefault();
      dropdownOpen = false;
      inputEl?.focus();
    } else if (e.metaKey && e.key >= '1' && e.key <= '9') {
      const idx = parseInt(e.key) - 1;
      const items = dropdownItems();
      if (idx < items.length) {
        e.preventDefault();
        items[idx].click();
      }
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const items = dropdownItems();
      if (items.length === 0) return;
      const idx = selectedIndex < items.length ? selectedIndex : -1;
      selectedIndex =
        e.key === 'ArrowDown' ? Math.min(idx + 1, items.length - 1) : Math.max(idx - 1, -1);
      items[selectedIndex]?.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter') {
      const items = dropdownItems();
      if (selectedIndex >= 0 && selectedIndex < items.length) {
        e.preventDefault();
        items[selectedIndex].click();
      }
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="repo-search-wrapper"
  bind:this={wrapperEl}
  onkeydown={handleKeydown}
  onfocusout={handleFocusOut}
>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="search-input" class:disabled bind:this={searchBoxEl} onclick={() => inputEl?.focus()}>
    <Search size={14} class="search-icon" />
    <Input
      bind:ref={inputEl}
      bind:value={query}
      type="text"
      placeholder="Search or paste a repository..."
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck="false"
      {disabled}
      oninput={handleInput}
      onfocus={handleFocus}
      role="combobox"
      aria-expanded={dropdownOpen}
      aria-autocomplete="list"
      aria-controls={dropdownOpen ? listboxId : undefined}
      aria-activedescendant={activeOptionId}
      class="border-0 bg-transparent shadow-none px-0 py-0 h-auto min-h-0 focus-visible:ring-0 focus-visible:border-0 text-base"
    />
  </div>

  {#if dropdownOpen}
    <!-- The listbox takes no focus: the field keeps it and names the
         highlighted option (ARIA combobox pattern). -->
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <!-- Portaled to #portal-root so position:fixed escapes the dialog's
         transformed containing block and the form's scroll clip. pointerdown
         is stopped so a tap on an item doesn't trip bits-ui's
         dismiss-on-interact-outside and close the dialog; mousedown is
         cancelled so the field keeps the focus: on iOS the focus would
         otherwise leave on the tap's mousedown, the focusout would close the
         dropdown, and the click would land on nothing.
         The highlight follows mousemove, not mouseenter: ArrowDown's
         scrollIntoView can move another item under a pointer that hasn't
         moved, and that item's mouseenter would snap the highlight back. -->
    <div
      id={listboxId}
      role="listbox"
      aria-label="Repositories"
      class="repo-dropdown"
      class:above={dropdownAbove}
      style={dropdownStyle}
      bind:this={dropdownEl}
      use:portal
      onpointerdown={(e) => e.stopPropagation()}
      onmousedown={(e) => e.preventDefault()}
      onmouseleave={() => (selectedIndex = -1)}
    >
      {#if filteredRecentRepos.length > 0}
        {#each filteredRecentRepos as recent, i}
          {@const hue = recentHue(recent)}
          <button
            type="button"
            id={listboxOptionId(listboxId, i)}
            role="option"
            aria-selected={i === selectedIndex}
            class="repo-item recent"
            class:selected={i === selectedIndex}
            tabindex="-1"
            onmousemove={() => (selectedIndex = i)}
            style="--recent-bg: {badge.badgeBg(hue, dark)}; --recent-bg-hover: {badge.badgeBgHover(
              hue,
              dark
            )}; --recent-fg: {badge.badgeFg(hue, dark)}; --recent-border: {badge.badgeBorder(
              hue,
              dark
            )};"
            onclick={() =>
              handleSelect({
                nameWithOwner: recent.githubRepo,
                subpath: recent.subpath ?? undefined,
              })}
          >
            <div class="repo-icon recent-icon">
              <Clock size={14} />
            </div>
            <div class="repo-info">
              <span class="repo-name">
                <RepoLabel githubRepo={recent.githubRepo} subpath={recent.subpath} />
              </span>
            </div>
            {#if viewport.showShortcutHints && i < 9}
              <span class="keyboard-shortcut">
                <Command size={10} />
                {i + 1}
              </span>
            {/if}
            <div class="repo-action">
              <Plus size={14} />
            </div>
          </button>
        {/each}
      {/if}

      {#if loading}
        <div class="loading-state">
          <Spinner size={16} />
        </div>
      {:else if error}
        <div class="error-state">{error}</div>
      {:else if isSearching}
        <div class="loading-state">
          <Spinner size={16} />
          <span>Searching...</span>
        </div>
      {:else if displayItems.length === 0 && filteredRecentRepos.length === 0}
        <div class="empty-state">
          {#if query.trim()}
            {#if isOwnerRepoFormat(query.trim())}
              <span>No repository found for "{query.trim()}"</span>
            {:else}
              <span>No matching repositories</span>
            {/if}
          {:else}
            <span>No repositories found</span>
          {/if}
        </div>
      {:else}
        {#each displayItems as item, i}
          {#if item.type === 'repo'}
            {@const repo = item.data as GitHubRepo}
            {@const itemIndex = filteredRecentRepos.length + i}
            <button
              type="button"
              id={listboxOptionId(listboxId, itemIndex)}
              role="option"
              aria-selected={itemIndex === selectedIndex}
              class="repo-item"
              class:selected={itemIndex === selectedIndex}
              tabindex="-1"
              onmousemove={() => (selectedIndex = itemIndex)}
              onclick={() => handleSelect({ nameWithOwner: repo.nameWithOwner })}
            >
              <div class="repo-icon">
                {#if repo.isPrivate}
                  <Lock size={14} />
                {:else}
                  <Globe size={14} />
                {/if}
              </div>
              <div class="repo-info">
                <span class="repo-name"><RepoLabel githubRepo={repo.nameWithOwner} /></span>
                {#if repo.description}
                  <span class="repo-description">{repo.description}</span>
                {/if}
              </div>
              <div class="repo-action">
                <Plus size={14} />
              </div>
            </button>
          {/if}
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .repo-search-wrapper {
    position: relative;
  }

  .search-input {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 42px;
    padding: 10px 14px;
    border: 1.5px solid var(--border-muted);
    border-radius: 10px;
    background: var(--bg-primary);
    transition: border-color 0.15s ease;
  }

  .search-input:focus-within {
    border-color: var(--ui-accent);
  }

  .search-input.disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  /* Fixed and portaled; top/left/width/max-height come from
     updateDropdownPosition. */
  .repo-dropdown {
    position: fixed;
    box-sizing: border-box;
    max-height: 280px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    box-shadow: var(--shadow-elevated);
    z-index: var(--z-index-floating);
    animation: dropdown-in 0.15s ease;
    /* A bits-ui popper left open sets `body { pointer-events: none }`; the
       dropdown is outside its host, so re-enable them on it. */
    pointer-events: auto;
  }

  .repo-dropdown.above {
    animation-name: dropdown-in-above;
  }

  @keyframes dropdown-in {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  @keyframes dropdown-in-above {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .loading-state,
  .empty-state,
  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 20px 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .error-state {
    color: var(--ui-danger);
    text-align: center;
  }

  .repo-item {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 10px 12px;
    background: none;
    border: none;
    border-radius: 8px;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .repo-item.selected,
  .repo-item:focus {
    background-color: var(--bg-hover);
    outline: none;
  }

  .repo-icon {
    color: var(--text-muted);
    flex-shrink: 0;
    display: flex;
  }

  .repo-item.recent {
    background: var(--recent-bg);
    border-left: 2px solid var(--recent-border);
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
  }

  .repo-item.recent.selected,
  .repo-item.recent:focus {
    background: var(--recent-bg-hover);
  }

  .repo-item.recent .repo-name {
    color: var(--recent-fg);
  }

  .recent-icon {
    color: var(--recent-fg);
  }

  .repo-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    text-align: left;
  }

  .repo-name {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-description {
    font-size: var(--size-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .keyboard-shortcut {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px 5px;
    background: var(--bg-primary);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    flex-shrink: 0;
    line-height: 1;
  }

  .repo-action {
    color: var(--text-faint);
    flex-shrink: 0;
    display: flex;
    transition: color 0.15s ease;
  }

  .repo-item.selected .repo-action,
  .repo-item:focus .repo-action {
    color: var(--text-muted);
  }
</style>
