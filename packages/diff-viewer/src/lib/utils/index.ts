export {
  getDisplayPath,
  getFilePath,
  getLineBoundary,
  getLanguageFromDiff,
  isBinaryDiff,
  getTextLines,
  referenceFileAsDiff,
} from './diffUtils';

export {
  buildFileEntries,
  buildTree,
  compactTree,
  formatLineRange,
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
  SYNTAX_THEMES,
  type Token,
  type HighlighterTheme,
  type SyntaxThemeName,
} from './highlighter';

export { ConnectorRendererCanvas, type CommentHighlightInfo } from './connectorRendererCanvas';
export { setupDiffKeyboardNav } from './diffKeyboard';
export { setupMarkdownScrollSync } from './markdownScrollSync';
export { sanitize } from './sanitize';
