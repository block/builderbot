<!--
  MarkdownWysiwygEditor.svelte — WYSIWYG markdown editing surface

  Wraps Milkdown's Crepe (ProseMirror) behind a deliberately small contract —
  an initial markdown value in, `getMarkdown()` out — so the library stays
  swappable without touching callers. Crepe and its theme are loaded lazily:
  they pull in ProseMirror and CodeMirror, which no other view needs.

  Crepe's structural CSS is imported as-is; every colour it reads comes from
  `--crepe-*` variables mapped to our theme tokens below (per AGENTS.md, no
  hardcoded colours).
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
            // Nothing in a note needs LaTeX or the LLM sidebar, and both are
            // heavy; the rest of Crepe's defaults map onto ordinary markdown.
            [Crepe.Feature.Latex]: false,
            [Crepe.Feature.AI]: false,
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
    --crepe-color-inline-code: var(--ui-danger);
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

  .editor-host :global(.milkdown .ProseMirror) {
    padding: 24px;
    outline: none;
  }
</style>
