<!--
  MarkdownWysiwygEditor.svelte — WYSIWYG markdown editing surface

  Wraps Milkdown's Crepe (ProseMirror) behind a deliberately small contract —
  an initial markdown value in, `getMarkdown()` out — so the library stays
  swappable without touching callers. Crepe and its theme are loaded lazily:
  they pull in ProseMirror and CodeMirror, which no other view needs.

  Crepe ships two separable layers: the live markdown formatting (typing `# `
  or `**bold**` and watching it take effect), which comes from Milkdown's
  commonmark/GFM presets and always loads, and a set of optional widgets —
  hover handles, slash menu, selection toolbar, table and image chrome. We
  want the first and not the second, so most feature flags are off below and
  the surface is left as a quiet page.

  Crepe's structural CSS is imported as-is; every colour it reads comes from
  `--crepe-*` variables mapped to our theme tokens below (per AGENTS.md, no
  hardcoded colours), and its document typography is resized to match how
  NoteModal renders the same markdown.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { Crepe as CrepeEditor } from '@milkdown/crepe';

  interface Props {
    /** Markdown the editor starts with. Read once, when the editor mounts. */
    value?: string;
    /** Shown while the document is empty. */
    placeholder?: string;
    /** Fires on every document change with the current markdown. */
    onChange?: (markdown: string) => void;
    /** Fires once the editor is mounted and focused, or with the load failure. */
    onReady?: (error: Error | null) => void;
  }

  let { value = '', placeholder = 'Write your note…', onChange, onReady }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let crepe: CrepeEditor | null = null;
  let loadError = $state<string | null>(null);

  // Mount once per host element. `value` is intentionally not a dependency:
  // the editor owns the document after mount, and re-seeding it mid-edit would
  // discard the user's cursor and undo history.
  $effect(() => {
    const root = host;
    if (!root) return;

    const initialValue = untrack(() => value);
    const initialPlaceholder = untrack(() => placeholder);
    let disposed = false;
    let instance: CrepeEditor | null = null;

    (async () => {
      try {
        const [{ Crepe }] = await Promise.all([
          import('@milkdown/crepe'),
          import('@milkdown/crepe/theme/common/style.css'),
        ]);
        if (disposed) return;

        const editor = new Crepe({
          root,
          defaultValue: initialValue,
          features: {
            // Off: every widget that puts editor UI on the page. Formatting
            // stays reachable through markdown syntax and the preset keymaps
            // (Mod-B, Mod-I, …), which are core, not part of these features.
            [Crepe.Feature.BlockEdit]: false, // per-block hover `+` / drag handle, `/` menu
            [Crepe.Feature.Toolbar]: false, // floating toolbar on text selection
            [Crepe.Feature.ImageBlock]: false, // upload / URL-import placeholder widget
            [Crepe.Feature.Table]: false, // cell handles and row/column buttons
            [Crepe.Feature.CodeMirror]: false, // language picker, plus a hardcoded dark theme
            [Crepe.Feature.Latex]: false,
            [Crepe.Feature.AI]: false,
            // Left on, all quiet: ListItem (`- [ ]` checkboxes), LinkTooltip
            // (the only way to edit an href without retyping the markdown),
            // Placeholder, and Cursor (drop / gap cursors).
          },
          featureConfigs: {
            [Crepe.Feature.Placeholder]: { text: initialPlaceholder },
          },
        });

        editor.on((listener) => {
          listener.markdownUpdated((_ctx, markdown) => onChange?.(markdown));
        });

        // Hand ownership to the cleanup before the first await, so unmounting
        // mid-create still tears the editor down — and does so exactly once.
        instance = editor;
        await editor.create();
        if (disposed) return;

        crepe = editor;
        focus();
        onReady?.(null);
      } catch (e) {
        if (disposed) return;
        const error = e instanceof Error ? e : new Error(String(e));
        loadError = error.message;
        onReady?.(error);
      }
    })();

    return () => {
      disposed = true;
      const live = instance;
      instance = null;
      crepe = null;
      // Crepe tears down asynchronously; nothing waits on it after unmount, and
      // a teardown that races an unfinished create has nowhere to report.
      if (live) void live.destroy().catch(() => {});
    };
  });

  /** Current document as markdown, or the initial value if the editor failed to load. */
  export function getMarkdown(): string {
    return crepe?.getMarkdown() ?? value;
  }

  export function focus() {
    host?.querySelector<HTMLElement>('.ProseMirror')?.focus();
  }
</script>

<div class="editor-shell">
  <div class="editor-host" bind:this={host}></div>
  {#if loadError}
    <p class="editor-error" role="alert">Could not load the editor: {loadError}</p>
  {/if}
</div>

<style>
  .editor-shell {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .editor-host {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    background: var(--bg-primary);
  }

  .editor-host::-webkit-scrollbar {
    width: 6px;
  }

  .editor-host::-webkit-scrollbar-track {
    background: transparent;
  }

  .editor-host::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 3px;
  }

  .editor-host::-webkit-scrollbar-thumb:hover {
    background: var(--scrollbar-thumb-hover-transparent);
  }

  .editor-error {
    margin: 0;
    padding: 12px 24px;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  /* Crepe reads every colour through these; map them onto our theme tokens so
     the editor follows light/dark with the rest of the app. */
  .editor-host :global(.milkdown) {
    --crepe-color-background: var(--bg-primary);
    --crepe-color-on-background: var(--text-primary);
    --crepe-color-surface: var(--bg-menu);
    --crepe-color-surface-low: var(--bg-elevated);
    --crepe-color-on-surface: var(--text-primary);
    --crepe-color-on-surface-variant: var(--text-muted);
    --crepe-color-outline: var(--border-muted);
    --crepe-color-primary: var(--ui-accent);
    --crepe-color-secondary: var(--bg-elevated);
    --crepe-color-on-secondary: var(--text-primary);
    --crepe-color-inverse: var(--bg-elevated);
    --crepe-color-on-inverse: var(--text-primary);
    /* NoteModal leaves inline code in body colour; Crepe tints it by default. */
    --crepe-color-inline-code: var(--text-primary);
    --crepe-color-error: var(--ui-danger);
    --crepe-color-hover: var(--bg-hover);
    --crepe-color-selected: var(--ui-selection);
    --crepe-color-inline-area: var(--bg-hover);

    --crepe-base-font-size: var(--size-sm);
    --crepe-font-title: var(--font-sans);
    --crepe-font-default: var(--font-sans);
    --crepe-font-code: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;

    --crepe-shadow-1: var(--shadow-elevated);
    --crepe-shadow-2: var(--shadow-overlay);

    height: 100%;
  }

  /* Typography pass. Crepe's defaults are sized for a standalone page editor
     — a 16px base, a 2.6em h1, 60px/120px page margins. Everything below
     resizes the document to NoteModal's `.markdown-content`, so writing a
     note looks close to reading one. (Those rules are Svelte-scoped to
     NoteModal, so parity here means restating them, not sharing them.) */
  .editor-host :global(.milkdown .ProseMirror) {
    padding: 24px;
    outline: none;
    line-height: 1.7;
  }

  .editor-host :global(.milkdown .ProseMirror p) {
    font-size: 1em;
    line-height: inherit;
    padding: 0;
    margin: 0 0 0.75em;
  }

  .editor-host :global(.milkdown .ProseMirror > :last-child) {
    margin-bottom: 0;
  }

  .editor-host :global(.milkdown .ProseMirror h1),
  .editor-host :global(.milkdown .ProseMirror h2),
  .editor-host :global(.milkdown .ProseMirror h3),
  .editor-host :global(.milkdown .ProseMirror h4),
  .editor-host :global(.milkdown .ProseMirror h5),
  .editor-host :global(.milkdown .ProseMirror h6) {
    font-weight: 600;
    line-height: 1.3;
    padding: 0;
    margin: 1em 0 0.5em;
  }

  .editor-host :global(.milkdown .ProseMirror > :first-child) {
    margin-top: 0;
  }

  .editor-host :global(.milkdown .ProseMirror h1) {
    font-size: 1.25em;
  }
  .editor-host :global(.milkdown .ProseMirror h2) {
    font-size: 1.15em;
  }
  .editor-host :global(.milkdown .ProseMirror h3) {
    font-size: 1.05em;
  }
  .editor-host :global(.milkdown .ProseMirror h4),
  .editor-host :global(.milkdown .ProseMirror h5),
  .editor-host :global(.milkdown .ProseMirror h6) {
    font-size: 1em;
  }

  .editor-host :global(.milkdown .ProseMirror ul),
  .editor-host :global(.milkdown .ProseMirror ol) {
    margin: 0.5em 0;
  }

  /* The ListItem feature draws the bullet/number itself, in a box sized for
     Crepe's larger base font; scale it to one line of ours so the marker sits
     on the item's first line instead of below it. */
  .editor-host :global(.milkdown .milkdown-list-item-block li) {
    gap: 6px;
  }

  .editor-host :global(.milkdown .milkdown-list-item-block li .label-wrapper),
  .editor-host :global(.milkdown .milkdown-list-item-block li .label-wrapper .label) {
    height: 1.7em;
    width: 1.5em;
    padding: 0;
  }

  /* List item content is a paragraph; its trailing margin would double the
     gap between items. */
  .editor-host :global(.milkdown .milkdown-list-item-block p) {
    margin: 0;
  }

  .editor-host :global(.milkdown .ProseMirror code) {
    display: inline;
    background: var(--bg-elevated);
    font-size: 0.9em;
    line-height: inherit;
    padding: 0.15em 0.35em;
    border-radius: 3px;
  }

  .editor-host :global(.milkdown .ProseMirror pre) {
    margin: 0.75em 0;
    padding: 0.75em;
    background: var(--bg-elevated);
    border-radius: 6px;
    font-family: var(--crepe-font-code);
    font-size: 0.85em;
    line-height: 1.5;
  }

  .editor-host :global(.milkdown .ProseMirror pre code) {
    background: none;
    font-size: 1em;
    padding: 0;
  }

  .editor-host :global(.milkdown .ProseMirror blockquote) {
    margin: 0.5em 0;
    padding-left: 0.75em;
    color: var(--text-muted);
  }

  .editor-host :global(.milkdown .ProseMirror blockquote::before) {
    top: 0;
    bottom: 0;
    width: 3px;
    border-radius: 0;
    background: var(--border-muted);
  }

  /* Crepe keeps the rule 1px tall and pads the rest, via background-clip. */
  .editor-host :global(.milkdown .ProseMirror hr) {
    margin: 0.5em 0;
    background-color: var(--border-subtle);
  }

  .editor-host :global(.milkdown .ProseMirror a) {
    text-decoration: none;
  }

  .editor-host :global(.milkdown .ProseMirror a:hover) {
    text-decoration: underline;
  }

  .editor-host :global(.milkdown .ProseMirror strong) {
    font-weight: 600;
  }

  /* Without the Table feature there is no cell chrome to style around, so
     tables need the plain borders NoteModal gives them. */
  .editor-host :global(.milkdown .ProseMirror table) {
    border-collapse: collapse;
    width: 100%;
    margin: 0.75em 0;
  }

  .editor-host :global(.milkdown .ProseMirror th),
  .editor-host :global(.milkdown .ProseMirror td) {
    border: 1px solid var(--border-subtle);
    padding: 6px 12px;
    text-align: left;
  }

  .editor-host :global(.milkdown .ProseMirror th) {
    font-weight: 600;
  }

  .editor-host :global(.milkdown .crepe-placeholder::before) {
    color: var(--text-faint);
  }
</style>
