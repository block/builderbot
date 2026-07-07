<script lang="ts">
  import type { RichToolItem } from '../acpTranscript';
  import { formatJson } from '../acpTranscript';
  import { isRecord, type ToolCallViewModel } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface MatchRow {
    key: string;
    location: string;
    snippet: string;
  }

  interface Props {
    item: RichToolItem;
    viewModel: ToolCallViewModel;
  }

  let { item, viewModel }: Props = $props();
  let matches = $derived(extractMatches(item.rawOutput));

  function extractMatches(rawOutput: unknown): MatchRow[] {
    const candidates = matchCandidates(rawOutput);
    return candidates
      .map((candidate, index) => {
        if (!isRecord(candidate)) return null;
        const path = firstString(candidate, ['path', 'file', 'file_path', 'filename']) ?? '';
        const line = firstNumber(candidate, ['line', 'lineNumber', 'line_number']);
        const snippet = firstText(candidate, ['snippet', 'text', 'content', 'match', 'line']);
        if (!path && !snippet) return null;
        return {
          key: `${path}:${line ?? ''}:${index}`,
          location: `${path || 'match'}${line !== null ? `:${line}` : ''}`,
          snippet,
        };
      })
      .filter((match): match is MatchRow => match !== null);
  }

  function matchCandidates(value: unknown): unknown[] {
    if (Array.isArray(value)) return value;
    if (!isRecord(value)) return [];
    for (const key of ['matches', 'results', 'items']) {
      const candidate = value[key];
      if (Array.isArray(candidate)) return candidate;
    }
    return [];
  }

  function firstString(input: Record<string, unknown>, keys: string[]): string | null {
    for (const key of keys) {
      const value = input[key];
      if (typeof value === 'string' && value.trim()) return value;
    }
    return null;
  }

  function firstText(input: Record<string, unknown>, keys: string[]): string {
    for (const key of keys) {
      const value = input[key];
      if (typeof value === 'string') return value;
      if (value !== undefined && value !== null && typeof value !== 'number')
        return formatJson(value);
    }
    return '';
  }

  function firstNumber(input: Record<string, unknown>, keys: string[]): number | null {
    for (const key of keys) {
      const value = input[key];
      if (typeof value === 'number' && Number.isFinite(value)) return value;
    }
    return null;
  }
</script>

<div class="tool-detail-stack">
  <div class="tool-field-list">
    {#if viewModel.metadata.targetPath}
      <span class="tool-field-label">Path</span>
      <span class="tool-field-value">{viewModel.metadata.targetPath}</span>
    {/if}
    {#if viewModel.metadata.query}
      <span class="tool-field-label">{viewModel.category === 'search' ? 'Query' : 'Selector'}</span>
      <span class="tool-field-value">{viewModel.metadata.query}</span>
    {/if}
  </div>

  {#if viewModel.metadata.locations.length > 0}
    <div class="tool-meta-row">
      {#each viewModel.metadata.locations as location}
        <span class="tool-chip">{location.display}</span>
      {/each}
    </div>
  {/if}

  {#if matches.length > 0}
    <section>
      <div class="tool-panel-label">Matches</div>
      <div class="tool-match-list">
        {#each matches as match}
          <div class="tool-match-row">
            <span class="tool-chip">{match.location}</span>
            <code class="tool-match-snippet">{match.snippet}</code>
          </div>
        {/each}
      </div>
    </section>
  {/if}

  <OutputSections
    {viewModel}
    includePrimary={matches.length === 0}
    includeRaw={matches.length === 0}
  />
</div>
