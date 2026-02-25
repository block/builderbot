# Diff Viewer Search Feature

The diff viewer package now includes a full-featured search capability for searching across diff files with keyboard navigation and sidebar integration.

## Features

- **Cross-file search**: Search across all files in the diff
- **Two search scopes**:
  - `all`: Search all lines in changed files
  - `changes`: Search only changed lines (diffs)
- **Keyboard shortcuts**:
  - `Cmd/Ctrl+F`: Open search
  - `Cmd/Ctrl+G`: Next search result
  - `Cmd/Ctrl+Shift+G`: Previous search result
- **Match highlighting**: Visual indicators for search matches (requires integration)
- **Result navigation**: Jump between results with wrap-around
- **Expandable results**: Show more/less results per file

## Components

### Core Files

1. **`utils/diffSearch.ts`** - Search logic
   - `findMatches()` - Find all occurrences in a file
   - `SearchMatch` type - Match location data

2. **`state/searchState.svelte.ts`** - State management
   - `createSearchState()` - Factory function for search state
   - Manages query, results, current position, loading state

3. **`components/CrossFileSearchBar.svelte`** - Search UI
   - Input with debounced search
   - Scope toggle (all/changes)
   - Match counter

4. **`components/SearchResultItem.svelte`** - Result item UI
   - Individual search result display
   - Highlights current result

## Integration Guide

### 1. Create Search State

In your app's diff modal/view component:

```typescript
import { createSearchState } from '@builderbot/diff-viewer/state';

// Create search state alongside diffViewerState and reviewState
const searchState = createSearchState();
```

### 2. Add Search Bar to Sidebar

```svelte
<script>
  import { CrossFileSearchBar } from '@builderbot/diff-viewer/components';
  import { searchState } from './your-state';

  // Your existing file list and loadFileDiff function
  let files = $state([]);
  async function loadFileDiff(path: string) { /* ... */ }
</script>

<!-- Add at top of sidebar -->
<CrossFileSearchBar
  files={files}
  loadFileDiff={loadFileDiff}
  searchState={searchState}
/>
```

### 3. Display Search Results in File List

For each file in your sidebar:

```svelte
{#each files as file}
  <div class="file-item">
    <span>{file.after ?? file.before}</span>

    <!-- Show search result count -->
    {#if searchState.state.fileResults.has(filePath)}
      {@const result = searchState.state.fileResults.get(filePath)}
      <span class="search-badge">{result.matches.length}</span>
    {/if}
  </div>

  <!-- Show search results for this file -->
  {#if searchState.state.isOpen && searchState.state.fileResults.has(filePath)}
    {@const result = searchState.state.fileResults.get(filePath)}
    <div class="search-results">
      {#each result.matches.slice(0, result.displayLimit) as match, i}
        <SearchResultItem
          match={match}
          snippet={getSnippet(match)}
          isCurrent={searchState.isCurrentResult(files, filePath, i)}
          onclick={() => handleClickResult(filePath, match)}
        />
      {/each}

      <!-- Show More/Less buttons -->
      {#if result.matches.length > result.displayLimit}
        <button onclick={() => searchState.expandFileResults(filePath)}>
          Show More ({result.matches.length - result.displayLimit} more)
        </button>
      {/if}
      {#if result.displayLimit > 5}
        <button onclick={() => searchState.collapseFileResults(filePath)}>
          Show Less
        </button>
      {/if}
    </div>
  {/if}
{/each}
```

### 4. Wire Up Keyboard Shortcuts

Update your `setupDiffKeyboardNav` call:

```typescript
import { setupDiffKeyboardNav } from '@builderbot/diff-viewer/utils';

// In your component
$effect(() => {
  const cleanup = setupDiffKeyboardNav({
    // ... existing config ...
    onOpenSearch: () => searchState.openSearch(),
    onNextSearchResult: async () => {
      const result = await searchState.goToNextResult(files);
      if (result) {
        // Load file and scroll to match
        await loadAndShowFile(result.filePath, result.match);
      }
    },
    onPrevSearchResult: async () => {
      const result = await searchState.goToPrevResult(files);
      if (result) {
        await loadAndShowFile(result.filePath, result.match);
      }
    },
  });

  return cleanup;
});
```

### 5. Helper Functions

You'll need to implement:

```typescript
// Get text snippet for a search match
function getSnippet(match: SearchMatch): string {
  const line = afterLines[match.lineIndex] || '';
  return line.trim().slice(0, 80); // Truncate long lines
}

// Handle clicking a search result
async function handleClickResult(filePath: string, match: SearchMatch) {
  // 1. Load the file if not already loaded
  await loadFileDiff(filePath);

  // 2. Switch to this file
  currentFilePath = filePath;

  // 3. Scroll to the line (if you have a scrollToRow function)
  scrollToRow(match.lineIndex, 'after');
}
```

## Search Highlighting (Optional)

For inline highlighting of search matches in the diff viewer, you need to integrate with the DiffViewer's token rendering. This is advanced and requires modifying how tokens are rendered.

See the old codebase (`~/Code/staged/src/lib/DiffViewer.svelte:745-892`) for reference implementation:
- `getSearchMatchesForLine()` - Get matches for a line
- `applySearchHighlights()` - Split tokens at match boundaries
- CSS classes: `.search-match` and `.search-current`

## CSS Variables

Add these to your theme:

```css
:root {
  --search-match-bg: rgba(250, 200, 50, 0.35); /* Yellow highlight */
  --search-current-match-bg: rgba(255, 150, 50, 0.5); /* Orange for current */
}
```

## API Reference

### SearchState

```typescript
interface SearchState {
  isOpen: boolean;
  query: string;
  scope: 'all' | 'changes';
  fileResults: Map<string, FileSearchResult>;
  currentResultIndex: number;
  totalMatches: number;
  loading: boolean;
}

// Methods
searchState.openSearch()
searchState.closeSearch()
searchState.setSearchScope(scope: 'all' | 'changes')
searchState.performSearch(query, files, loadFileDiff)
searchState.goToNextResult(files)
searchState.goToPrevResult(files)
searchState.expandFileResults(filePath)
searchState.collapseFileResults(filePath)
searchState.isCurrentResult(files, filePath, localIndex)
```

### SearchMatch

```typescript
interface SearchMatch {
  lineIndex: number;        // Line index in the file
  right: MatchLocation;     // Column range of match
}

interface MatchLocation {
  startCol: number;
  endCol: number;
}
```

## Example Apps

See how Mark and Staged apps integrate search:
- Mark: `apps/mark/src/lib/features/diff/DiffModal.svelte`
- Staged: `apps/staged/src/App.svelte`
