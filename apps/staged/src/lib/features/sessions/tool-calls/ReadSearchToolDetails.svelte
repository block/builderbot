<script lang="ts">
  import type { RichToolItem } from '../acpTranscript';
  import { formatJson } from '../acpTranscript';
  import type { DisplayRootInput } from '../pathDisplayRoots';
  import { makePathsRelative } from '../sessionModalHelpers';
  import {
    isRecord,
    summarizeToolCallLocations,
    type ToolCallViewModel,
  } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface MatchRow {
    key: string;
    location: string;
    snippet: string;
  }

  interface Props {
    item: RichToolItem;
    viewModel: ToolCallViewModel;
    displayRoots?: DisplayRootInput;
  }

  let { item, viewModel, displayRoots }: Props = $props();
  let matches = $derived(extractMatches(item.rawOutput, displayRoots));
  // The Path field names the file (with its line suffix folded in); chips only
  // carry locations it doesn't already cover.
  let locationSummary = $derived(
    summarizeToolCallLocations(viewModel.metadata.locations, viewModel.metadata.targetPath)
  );

  function extractMatches(rawOutput: unknown, roots?: DisplayRootInput): MatchRow[] {
    const candidates = matchCandidates(rawOutput);
    return candidates
      .map((candidate, index) => {
        if (!isRecord(candidate)) return null;
        const rawPath = firstString(candidate, ['path', 'file', 'file_path', 'filename']) ?? '';
        const path = rawPath ? makePathsRelative(rawPath, roots) : '';
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
      <span class="tool-field-value"
        >{viewModel.metadata.targetPath}{locationSummary.pathSuffix}</span
      >
    {/if}
    {#if viewModel.metadata.query}
      <span class="tool-field-label">{viewModel.category === 'search' ? 'Query' : 'Selector'}</span>
      <span class="tool-field-value">{viewModel.metadata.query}</span>
    {/if}
  </div>

  {#if locationSummary.chips.length > 0}
    <div class="tool-meta-row">
      {#each locationSummary.chips as chip}
        <span class="tool-chip">{chip}</span>
      {/each}
    </div>
  {/if}

  {#if matches.length > 0}
    <section>
      <div class="tool-panel-label">
        {matches.length}
        {matches.length === 1 ? 'match' : 'matches'}
      </div>
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
