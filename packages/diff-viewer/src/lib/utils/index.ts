export {
  getDisplayPath,
  getFilePath,
  getLineBoundary,
  getLanguageFromDiff,
  isBinaryDiff,
  isImageDiff,
  getTextLines,
  referenceFileAsDiff,
} from './diffUtils';

export {
  buildFileEntries,
  buildTree,
  compactTree,
  formatLineRange,
  pathsMatch,
  truncateText,
  type FileEntry,
  type TreeNode,
} from './diffModalHelpers';

export {
  buildLineToAlignmentMap,
  buildBeforeMarkers,
  buildAfterMarkers,
  findCommentById,
  getCommentsForAlignment,
  getTokensForLine,
  isLineInChangedAlignment,
  isLineInIndexedRange,
  isLineSelected,
  buildLineCommentEditorLayout,
  buildLineSelectionToolbarLayout,
  buildRangeCommentEditorLayout,
  decideCommentPositionBySpace,
  measureContentWidth,
  measureLineHeight,
  normalizeLineSelection,
  resolveLineSelectionToolbarLeft,
  getLineClass,
  getCharHighlights,
} from './diffViewerHelpers';

export {
  initHighlighter,
  highlightLines,
  highlightLine,
  detectLanguage,
  prepareLanguage,
  getTheme,
  getSyntaxThemeName,
  setSyntaxTheme,
  isLightTheme,
  registerExternalThemes,
  getRegisteredThemes,
  hasTheme,
  SYNTAX_THEMES,
  type Token,
  type HighlighterTheme,
  type SyntaxThemeName,
} from './highlighter';

export { ConnectorRendererCanvas, type CommentHighlightInfo } from './connectorRendererCanvas';
export { setupDiffKeyboardNav } from './diffKeyboard';
export { setupMarkdownScrollSync } from './markdownScrollSync';
export { sanitize } from './sanitize';

export {
	findMatches,
	describeMatch,
	getMatchSnippet,
	MAX_MATCHES,
	type SearchMatch,
	type MatchLocation
} from './diffSearch';

export {
	createSearchNavigationHandlers,
	type SearchNavigationConfig
} from './searchNavigation';

export { createSearchInitializationTracker, type SearchInitializationConfig } from './searchInitialization';

export {
	createFileSelectionWithSearch,
	type FileSelectionWithSearchConfig
} from './fileSelection';

export { computeLineDiff, createLineDiffCache } from './inlineDiff.js';
export type { BeforeLineClass, AfterLineClass, CharHighlight, ModifiedPair, LineDiffResult, LineDiffCache } from './inlineDiff.js';
