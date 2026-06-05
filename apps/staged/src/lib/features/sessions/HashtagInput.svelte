<!--
  HashtagInput.svelte — Contenteditable editor with inline hashtag badges

  Wraps a contenteditable <div> and adds:
  - Dropdown triggered by typing `#` (filtered by subsequent chars)
  - Inline badge spans replacing #type:id tokens within the text
  - Keyboard navigation (Arrow keys, Enter, Escape)

  Props:
    value      — bindable raw text including #type:id tokens (for backend consumption)
    placeholder
    disabled
    items      — full list of HashtagItems (parent provides, scoped correctly)
    class      — pass-through for styling
    rows       — approximate initial height in rows
    onkeydown  — pass-through keydown handler (called after hashtag handling)
    oninput    — pass-through input handler
    onpaste    — pass-through paste handler (e.g. for image paste)
    textareaEl — bindable element reference (the contenteditable div)
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { HashtagItem } from '../../types';
  import FileText from '@lucide/svelte/icons/file-text';
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import ImageLucide from '@lucide/svelte/icons/image';
  import { HASHTAG_TOKEN_RE, hashtagTypeIconSvg, escapeHtml } from './hashtagItems';
  import { focusAtEndSync } from '../../shared/focusAtEnd';
  import { portal } from '../../shared/portal';
  import RepoLabel from '../../shared/RepoLabel.svelte';

  type DropdownIconComponent = typeof FileText;

  /** Component lookup map for dropdown icons — keep in sync with hashtagTypeIconSvg SVG strings. */
  const dropdownIconMap: Record<string, DropdownIconComponent> = {
    note: FileText,
    commit: GitCommitVertical,
    review: FileSearch,
    'project-note': FileText,
    image: ImageLucide,
  };

  interface Props {
    value: string;
    placeholder?: string;
    disabled?: boolean;
    items: HashtagItem[];
    class?: string;
    rows?: number | string;
    onkeydown?: (e: KeyboardEvent) => void;
    oninput?: (e: Event) => void;
    onpaste?: (e: ClipboardEvent) => void;
    textareaEl?: HTMLElement | null;
  }

  let {
    value = $bindable(''),
    placeholder = '',
    disabled = false,
    items = [],
    class: className = '',
    rows = 1,
    onkeydown,
    oninput,
    onpaste,
    textareaEl = $bindable(null),
  }: Props = $props();

  let editorEl: HTMLDivElement | undefined = $state();
  let showDropdown = $state(false);
  let filterText = $state('');
  let selectedIndex = $state(0);
  let dropdownEl: HTMLDivElement | undefined = $state();
  let wrapperEl: HTMLDivElement | undefined = $state();
  let dropdownPosition = $state<'above' | 'below'>('below');
  let dropdownStyle = $state('');
  let pendingInsert: { textNode: Text; hashPos: number; cursorPos: number } | null = null;

  type HashtagSection = {
    key: string;
    label?: string;
    repoSlug?: string;
    repoSubpath?: string | null;
    items: HashtagItem[];
    startIndex: number;
  };

  // Sync editorEl to the textareaEl binding
  $effect(() => {
    textareaEl = editorEl ?? null;
  });

  let itemsById = $derived.by(() => {
    const map = new Map<string, HashtagItem>();
    for (const item of items) {
      map.set(`${item.type}:${item.id}`, item);
    }
    return map;
  });

  let selectedTokenKeys = $derived.by(() => {
    const keys = new Set<string>();
    const regex = new RegExp(HASHTAG_TOKEN_RE.source, 'g');
    let m;
    while ((m = regex.exec(value)) !== null) {
      keys.add(`${m[1]}:${m[2]}`);
    }
    return keys;
  });

  let filteredSections = $derived.by((): HashtagSection[] => {
    if (!showDropdown) return [];
    const filter = filterText.toLowerCase();
    const sectionsByKey = new Map<string, Omit<HashtagSection, 'startIndex'>>();

    for (const item of items) {
      if (selectedTokenKeys.has(`${item.type}:${item.id}`)) continue;
      if (!item.title.toLowerCase().includes(filter)) continue;

      const sectionKey = hashtagSectionKey(item);
      const section = sectionsByKey.get(sectionKey);
      if (section) {
        section.items.push(item);
      } else {
        sectionsByKey.set(sectionKey, {
          key: sectionKey,
          ...hashtagSectionLabel(item),
          items: [item],
        });
      }
    }

    const sections: HashtagSection[] = [];
    let startIndex = 0;
    for (const section of sectionsByKey.values()) {
      sections.push({
        ...section,
        startIndex,
      });
      startIndex += section.items.length;
    }
    return sections;
  });

  let filteredItems = $derived.by(() => filteredSections.flatMap((section) => section.items));

  function hashtagSectionKey(item: HashtagItem): string {
    if (item.type === 'project-note') return 'project-notes';
    if (item.repoSlug) return `repo:${item.repoSlug}\u0000${item.repoSubpath ?? ''}`;
    return 'branch-references';
  }

  function hashtagSectionLabel(
    item: HashtagItem
  ): Pick<HashtagSection, 'label' | 'repoSlug' | 'repoSubpath'> {
    if (item.type === 'project-note') return { label: 'Project notes' };
    if (item.repoSlug) return { repoSlug: item.repoSlug, repoSubpath: item.repoSubpath };
    return { label: 'Branch references' };
  }

  $effect(() => {
    const itemCount = filteredItems.length;
    if (itemCount === 0) {
      if (selectedIndex !== 0) selectedIndex = 0;
      return;
    }
    if (selectedIndex >= itemCount) selectedIndex = itemCount - 1;
  });

  let lastExtractedValue = '';
  let hasUnresolvedTokens = false;

  // When value changes from outside, re-render the editor content
  $effect(() => {
    if (value !== lastExtractedValue && editorEl) {
      renderContent(value);
      lastExtractedValue = value;
    }
  });

  // When items become available, re-render to resolve plain-text tokens into badges
  $effect(() => {
    const _items = itemsById;
    if (hasUnresolvedTokens && _items.size > 0 && editorEl) {
      const selKind = classifySelection(editorEl);
      const sel = window.getSelection();

      renderContent(value);

      if (selKind === 'all') {
        const range = document.createRange();
        range.selectNodeContents(editorEl);
        if (sel) {
          sel.removeAllRanges();
          sel.addRange(range);
        }
      } else if (selKind === 'last-line') {
        // Restore selection from after the last <br> to end of content
        const start = findLastBrStart(editorEl);
        if (start && sel) {
          const range = document.createRange();
          range.setStart(start.node, start.offset);
          range.setEnd(editorEl, editorEl.childNodes.length);
          sel.removeAllRanges();
          sel.addRange(range);
        }
      } else if (selKind === 'caret') {
        // Restore collapsed cursor to end after re-render.
        // DOM is already fresh (renderContent just ran), so use the sync variant.
        focusAtEndSync(editorEl);
      }
    }
  });

  function classifySelection(root: HTMLElement): 'all' | 'last-line' | 'caret' | 'none' {
    const sel = window.getSelection();
    if (!sel || !sel.rangeCount || !sel.anchorNode || !root.contains(sel.anchorNode)) {
      return 'none';
    }
    if (sel.isCollapsed) return 'caret';
    if (!sel.focusNode || !root.contains(sel.focusNode)) return 'none';

    const selRange = sel.getRangeAt(0);
    const fullRange = document.createRange();
    fullRange.selectNodeContents(root);

    const startsAtBeginning = selRange.compareBoundaryPoints(Range.START_TO_START, fullRange) === 0;
    const endsAtEnd = selRange.compareBoundaryPoints(Range.END_TO_END, fullRange) === 0;

    if (startsAtBeginning && endsAtEnd) return 'all';
    if (endsAtEnd && !startsAtBeginning) return 'last-line';
    return 'none';
  }

  function findLastBrStart(root: HTMLElement): { node: Node; offset: number } | null {
    let result: { node: Node; offset: number } | null = null;
    for (let i = 0; i < root.childNodes.length; i++) {
      if (root.childNodes[i] instanceof HTMLBRElement) {
        result = { node: root, offset: i + 1 };
      }
    }
    return result;
  }

  function createBadgeElement(item: HashtagItem): HTMLSpanElement {
    const badge = document.createElement('span');
    badge.className = 'hashtag-badge';
    badge.contentEditable = 'false';
    badge.dataset.token = `#${item.type}:${item.id}`;
    const iconSvg = hashtagTypeIconSvg[item.type] ?? '';
    badge.innerHTML = `${iconSvg} ${escapeHtml(item.title)}`;
    badge.style.cssText = `background: var(${item.bgColor}); color: var(${item.color});`;
    return badge;
  }

  function renderContent(val: string) {
    if (!editorEl) return;
    editorEl.innerHTML = '';

    const regex = new RegExp(HASHTAG_TOKEN_RE.source, 'g');
    let lastIndex = 0;
    let match;
    let unresolved = false;

    while ((match = regex.exec(val)) !== null) {
      if (match.index > lastIndex) {
        appendTextNodes(editorEl, val.slice(lastIndex, match.index));
      }

      const type = match[1];
      const id = match[2];
      const item = itemsById.get(`${type}:${id}`);
      if (item) {
        editorEl.appendChild(createBadgeElement(item));
        // Zero-width space so cursor can be placed after badge
        editorEl.appendChild(document.createTextNode('\u200B'));
      } else {
        editorEl.appendChild(document.createTextNode(match[0]));
        unresolved = true;
      }

      lastIndex = match.index + match[0].length;
    }

    if (lastIndex < val.length) {
      appendTextNodes(editorEl, val.slice(lastIndex));
    }

    hasUnresolvedTokens = unresolved;
  }

  function appendTextNodes(parent: HTMLElement, text: string) {
    const lines = text.split('\n');
    for (let i = 0; i < lines.length; i++) {
      if (i > 0) parent.appendChild(document.createElement('br'));
      if (lines[i]) parent.appendChild(document.createTextNode(lines[i]));
    }
  }

  function extractValue(): string {
    if (!editorEl) return '';
    return extractFromNode(editorEl);
  }

  function extractFromNode(node: Node): string {
    let result = '';
    for (const child of node.childNodes) {
      if (child.nodeType === Node.TEXT_NODE) {
        result += (child.textContent ?? '').replace(/\u200B/g, '');
      } else if (child instanceof HTMLBRElement) {
        result += '\n';
      } else if (child instanceof HTMLElement) {
        if (child.dataset.token) {
          result += child.dataset.token;
        } else if (child.tagName === 'DIV') {
          // Browsers sometimes wrap lines in <div> elements on Enter
          const innerText = extractFromNode(child);
          if (result.length > 0 && !result.endsWith('\n')) {
            result += '\n';
          }
          result += innerText;
        } else {
          result += extractFromNode(child);
        }
      }
    }
    return result;
  }

  function handleInput(e: Event) {
    const extracted = extractValue();
    lastExtractedValue = extracted;
    value = extracted;
    detectHashtag();
    oninput?.(e);
  }

  function detectHashtag() {
    const sel = window.getSelection();
    if (!sel || !sel.rangeCount || !editorEl) return;

    const range = sel.getRangeAt(0);
    if (!range.collapsed) {
      closeDropdown();
      return;
    }

    const node = range.startContainer;
    if (node.nodeType !== Node.TEXT_NODE || !editorEl.contains(node)) {
      closeDropdown();
      return;
    }

    const text = node.textContent ?? '';
    const pos = range.startOffset;

    let hashPos = -1;
    for (let i = pos - 1; i >= 0; i--) {
      const ch = text[i];
      if (ch === '#') {
        if (i > 0 && /\w/.test(text[i - 1])) break;
        hashPos = i;
        break;
      }
      if (ch === ' ' || ch === '\n' || ch === '\r' || ch === '\t') break;
    }

    if (hashPos >= 0) {
      const fragment = text.slice(hashPos + 1, pos);
      if (new RegExp(HASHTAG_TOKEN_RE.source).test('#' + fragment)) {
        closeDropdown();
        return;
      }
      showDropdown = true;
      filterText = fragment;
      selectedIndex = 0;
      pendingInsert = { textNode: node as Text, hashPos, cursorPos: pos };
      updateDropdownPosition();
    } else {
      closeDropdown();
    }
  }

  function updateDropdownPosition() {
    const sel = window.getSelection();
    if (!sel || !sel.rangeCount) return;

    const range = sel.getRangeAt(0);
    const rect = range.getBoundingClientRect();
    const editorRect = editorEl?.getBoundingClientRect();
    if (!editorRect) return;

    const caretTop = rect.top || editorRect.top;
    const caretBottom = rect.bottom || editorRect.top + 20;
    const caretLeft = rect.left || editorRect.left;

    const maxHeight = 280;
    const spaceBelow = window.innerHeight - caretBottom;
    const spaceAbove = caretTop;

    if (spaceBelow >= maxHeight || spaceBelow > spaceAbove) {
      dropdownPosition = 'below';
      dropdownStyle = `position:fixed; top:${caretBottom + 4}px; left:${caretLeft}px;`;
    } else {
      dropdownPosition = 'above';
      dropdownStyle = `position:fixed; bottom:${window.innerHeight - caretTop + 4}px; left:${caretLeft}px;`;
    }
  }

  function closeDropdown() {
    showDropdown = false;
    filterText = '';
    selectedIndex = 0;
    pendingInsert = null;
  }

  function selectItem(item: HashtagItem) {
    if (!editorEl || !pendingInsert) return;

    const { textNode, hashPos, cursorPos } = pendingInsert;

    if (!editorEl.contains(textNode)) {
      closeDropdown();
      return;
    }

    const text = textNode.textContent ?? '';
    const before = text.slice(0, hashPos);
    const after = text.slice(cursorPos);

    // Replace the #filter text with a badge
    textNode.textContent = before;

    const badge = createBadgeElement(item);
    const afterNode = document.createTextNode('\u200B' + after);

    const parent = textNode.parentNode!;
    const nextSibling = textNode.nextSibling;
    parent.insertBefore(badge, nextSibling);
    parent.insertBefore(afterNode, badge.nextSibling);

    // Place cursor after the badge
    const sel = window.getSelection();
    if (sel) {
      const range = document.createRange();
      range.setStart(afterNode, 1);
      range.collapse(true);
      sel.removeAllRanges();
      sel.addRange(range);
    }

    closeDropdown();

    const extracted = extractValue();
    lastExtractedValue = extracted;
    value = extracted;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (showDropdown && filteredItems.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        selectedIndex = (selectedIndex + 1) % filteredItems.length;
        scrollSelectedIntoView();
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        selectedIndex = (selectedIndex - 1 + filteredItems.length) % filteredItems.length;
        scrollSelectedIntoView();
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey && !e.metaKey) {
        e.preventDefault();
        e.stopPropagation();
        selectItem(filteredItems[selectedIndex]);
        return;
      }
    }

    if (e.key === 'Escape' && showDropdown) {
      e.preventDefault();
      e.stopPropagation();
      closeDropdown();
      return;
    }

    onkeydown?.(e);
  }

  function handlePaste(e: ClipboardEvent) {
    if (onpaste) {
      onpaste(e);
      if (e.defaultPrevented) return;
    }

    // Strip HTML and insert plain text
    e.preventDefault();
    const text = e.clipboardData?.getData('text/plain') ?? '';
    document.execCommand('insertText', false, text);
  }

  function scrollSelectedIntoView() {
    tick().then(() => {
      if (!dropdownEl) return;
      const selected = dropdownEl.querySelector('.hashtag-dropdown-item.selected');
      if (selected) {
        selected.scrollIntoView({ block: 'nearest' });
      }
    });
  }

  function handleBlur(e: FocusEvent) {
    if (wrapperEl && e.relatedTarget instanceof Node && wrapperEl.contains(e.relatedTarget)) {
      return;
    }
    closeDropdown();
  }
</script>

<div class="hashtag-input-wrapper" bind:this={wrapperEl}>
  <div class="hashtag-input-container">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div
      bind:this={editorEl}
      class="hashtag-editor {className}"
      class:is-empty={!value}
      contenteditable={disabled ? 'false' : 'true'}
      role="textbox"
      aria-multiline="true"
      data-placeholder={placeholder}
      style="min-height: {Number(rows) * 1.5}em"
      oninput={handleInput}
      onkeydown={handleKeydown}
      onblur={handleBlur}
      onpaste={handlePaste}
    ></div>

    <!-- Dropdown -->
    {#if showDropdown && filteredItems.length > 0}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <!-- Portaled to <body> so position:fixed escapes the dialog's transformed
           containing block; pointerdown is stopped so selecting an item doesn't
           trip bits-ui's dismiss-on-interact-outside and close the dialog. -->
      <div
        class="hashtag-dropdown"
        class:above={dropdownPosition === 'above'}
        class:below={dropdownPosition === 'below'}
        style={dropdownStyle}
        bind:this={dropdownEl}
        use:portal
        onpointerdown={(e) => e.stopPropagation()}
      >
        {#each filteredSections as section (section.key)}
          <div class="hashtag-dropdown-section">
            <div class="hashtag-section-header">
              {#if section.repoSlug}
                <span class="hashtag-section-repo"
                  ><RepoLabel githubRepo={section.repoSlug} subpath={section.repoSubpath} /></span
                >
              {:else}
                <span class="hashtag-section-label">{section.label}</span>
              {/if}
            </div>
            {#each section.items as item, i}
              {@const Icon = dropdownIconMap[item.type]}
              {@const itemIndex = section.startIndex + i}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="hashtag-dropdown-item"
                class:selected={itemIndex === selectedIndex}
                onmousedown={(e) => {
                  e.preventDefault();
                  selectItem(item);
                }}
                onmouseenter={() => (selectedIndex = itemIndex)}
              >
                <span class="hashtag-item-icon {item.type}-icon">
                  {#if Icon}
                    <Icon size={14} />
                  {/if}
                </span>
                <span class="hashtag-item-text">
                  <span class="hashtag-item-title">{item.title}</span>
                </span>
              </div>
            {/each}
          </div>
        {/each}
      </div>
    {:else if showDropdown && filterText.length > 0}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="hashtag-dropdown"
        class:above={dropdownPosition === 'above'}
        class:below={dropdownPosition === 'below'}
        style={dropdownStyle}
        use:portal
        onpointerdown={(e) => e.stopPropagation()}
      >
        <div class="hashtag-dropdown-empty">No matching items</div>
      </div>
    {/if}
  </div>
</div>

<style>
  .hashtag-input-wrapper {
    position: relative;
  }

  .hashtag-input-container {
    position: relative;
  }

  /* Contenteditable editor styled like a textarea */
  .hashtag-editor {
    position: relative;
    width: 100%;
    box-sizing: border-box;
    white-space: pre-wrap;
    word-wrap: break-word;
    overflow-wrap: break-word;
    overflow-y: hidden;
    outline: none;
    line-height: 1.5;
  }

  /* Placeholder via ::before when empty */
  .hashtag-editor.is-empty::before {
    content: attr(data-placeholder);
    color: var(--text-faint);
    pointer-events: none;
    position: absolute;
    top: 0;
    left: 0;
    padding: inherit;
  }

  /* Inline badge for referenced items */
  .hashtag-editor :global(.hashtag-badge) {
    cursor: default;
    vertical-align: baseline;
    user-select: all;
  }

  /* Dropdown — fixed-positioned near the caret via inline style */
  .hashtag-dropdown {
    width: 420px;
    max-width: calc(100vw - 32px);
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    max-height: 280px;
    overflow-y: auto;
    z-index: var(--z-index-floating);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    padding: 4px;
  }

  .hashtag-dropdown-section + .hashtag-dropdown-section {
    margin-top: 4px;
  }

  .hashtag-section-header {
    display: flex;
    min-width: 0;
    padding: 6px 10px 3px;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-muted);
  }

  .hashtag-section-label,
  .hashtag-section-repo {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hashtag-section-repo :global(.repo-label) {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hashtag-dropdown-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.1s;
    overflow: hidden;
  }

  .hashtag-dropdown-item:hover,
  .hashtag-dropdown-item.selected {
    background: var(--bg-hover);
  }

  .hashtag-item-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .hashtag-item-icon.note-icon,
  .hashtag-item-icon.project-note-icon {
    background-color: var(--note-bg);
    color: var(--note-color);
  }

  .hashtag-item-icon.commit-icon {
    background-color: var(--commit-bg);
    color: var(--commit-color);
  }

  .hashtag-item-icon.review-icon {
    background-color: var(--review-bg);
    color: var(--review-color);
  }

  .hashtag-item-icon.image-icon {
    background-color: var(--image-bg);
    color: var(--image-color);
  }

  .hashtag-item-text {
    flex: 1;
    display: flex;
    overflow: hidden;
    min-width: 0;
  }

  .hashtag-item-title {
    font-size: var(--size-sm);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hashtag-dropdown-empty {
    padding: 8px 12px;
    font-size: var(--size-sm);
    color: var(--text-faint);
    text-align: center;
  }
</style>
