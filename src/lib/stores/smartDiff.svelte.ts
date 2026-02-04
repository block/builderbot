/**
 * Smart Diff Store - State management for AI-powered diff analysis.
 *
 * Uses a unified approach: the AI sees all files together and produces
 * both summary and per-file annotations in a single call. This provides
 * better context than analyzing files individually.
 *
 * Results are persisted to the database and loaded when switching diffs.
 */

import type { SmartDiffResult, ChangesetSummary, DiffSpec, SmartDiffAnnotation } from '../types';
import {
  analyzeDiff,
  checkAiAvailable,
  saveChangesetSummary,
  getChangesetSummary,
  saveFileAnalysis,
  getAllFileAnalyses,
  deleteAllAnalyses,
  saveAiComments,
} from '../services/ai';
import { preferences } from './preferences.svelte';

// =============================================================================
// State
// =============================================================================

interface SmartDiffState {
  /** AI analysis results keyed by file path */
  results: Map<string, SmartDiffResult>;
  /** Whether analysis is currently running */
  loading: boolean;
  /** Whether AI is available (goose or claude installed) */
  aiAvailable: boolean | null;
  /** Name of available AI tool */
  aiToolName: string | null;
  /** Error message if AI check failed */
  aiError: string | null;
  /** Error message if analysis failed */
  analysisError: string | null;
  /** Global toggle for showing annotations */
  showAnnotations: boolean;
  /** Currently focused annotation ID (for keyboard nav) */
  activeAnnotationId: string | null;
  /** Changeset-level summary (across all files) */
  changesetSummary: ChangesetSummary | null;
  /** Whether annotations are currently revealed (hold A key) */
  annotationsRevealed: boolean;
}

export const smartDiffState: SmartDiffState = $state({
  results: new Map(),
  loading: false,
  aiAvailable: null,
  aiToolName: null,
  aiError: null,
  analysisError: null,
  showAnnotations: true,
  activeAnnotationId: null,
  changesetSummary: null,
  annotationsRevealed: false,
});

// =============================================================================
// AI Availability
// =============================================================================

/**
 * Check if an AI CLI tool is available.
 * Caches the result for the session.
 */
export async function checkAi(): Promise<boolean> {
  if (smartDiffState.aiAvailable !== null) {
    return smartDiffState.aiAvailable;
  }

  try {
    const toolName = await checkAiAvailable();
    smartDiffState.aiAvailable = true;
    smartDiffState.aiToolName = toolName;
    smartDiffState.aiError = null;
    return true;
  } catch (e) {
    smartDiffState.aiAvailable = false;
    smartDiffState.aiToolName = null;
    smartDiffState.aiError = e instanceof Error ? e.message : String(e);
    return false;
  }
}

// =============================================================================
// Current Context (for persistence)
// =============================================================================
// Unified Changeset Analysis
// =============================================================================

/**
 * Analyze a diff with AI.
 *
 * The backend handles file listing and content loading - we just provide
 * the diff spec. Returns summary, key changes, concerns, and per-file annotations.
 *
 * Note: This function does NOT reload comments after saving AI comments.
 * The caller is responsible for reloading comments if needed (to handle tab switching).
 *
 * @param repoPath - Path to the repository (null for current directory)
 * @param spec - The diff specification (base..head)
 * @returns The summary, or null if analysis failed
 */
export async function runAnalysis(
  repoPath: string | null,
  spec: DiffSpec
): Promise<ChangesetSummary | null> {
  // Clear any previous analysis error
  smartDiffState.analysisError = null;

  // Check AI availability first
  const available = await checkAi();
  if (!available) {
    return null;
  }

  smartDiffState.loading = true;

  try {
    // Single backend call handles everything
    // Use the user's preferred AI agent if set
    const result = await analyzeDiff(repoPath, spec, preferences.aiAgent);

    // Store changeset summary
    const summary: ChangesetSummary = {
      summary: result.summary,
      key_changes: result.key_changes,
      concerns: result.concerns,
    };
    smartDiffState.changesetSummary = summary;

    // Store per-file results
    const newResults = new Map<string, SmartDiffResult>();
    for (const [filePath, annotations] of Object.entries(result.file_annotations)) {
      newResults.set(filePath, {
        overview: '', // Per-file overview not used in unified model
        annotations,
      });
    }
    smartDiffState.results = newResults;

    // Persist to database
    try {
      await saveChangesetSummary(repoPath, spec, summary);

      // Persist each file's annotations
      for (const [filePath, fileResult] of newResults) {
        await saveFileAnalysis(repoPath, spec, filePath, fileResult);
      }

      // Convert actionable annotations (warnings/suggestions) to persistent comments
      // Informational annotations (explanations/context) remain as blur overlays only
      const allAnnotations: SmartDiffAnnotation[] = [];
      for (const annotations of Object.values(result.file_annotations)) {
        allAnnotations.push(...annotations);
      }

      if (allAnnotations.length > 0) {
        // Backend filters to only save warnings and suggestions as comments
        await saveAiComments(repoPath, spec, allAnnotations);

        // NOTE: We intentionally do NOT call loadComments here.
        // The caller (TopBar.svelte) handles reloading comments to ensure
        // they go to the correct tab even if the user switched tabs during analysis.
      }
    } catch (e) {
      console.error('Failed to persist analysis:', e);
    }

    return summary;
  } catch (e) {
    console.error('Failed to analyze diff:', e);
    smartDiffState.analysisError = e instanceof Error ? e.message : String(e);
    return null;
  } finally {
    smartDiffState.loading = false;
  }
}

/**
 * Get the analysis result for a file, if available.
 */
export function getFileResult(filePath: string): SmartDiffResult | undefined {
  return smartDiffState.results.get(filePath);
}

/**
 * Check if analysis is currently running.
 */
export function isLoading(): boolean {
  return smartDiffState.loading;
}

// =============================================================================
// Display Controls
// =============================================================================

/**
 * Toggle annotation visibility globally.
 */
export function toggleAnnotations(): void {
  smartDiffState.showAnnotations = !smartDiffState.showAnnotations;
}

/**
 * Set whether annotations are revealed (code visible through blur).
 * Called on keydown/keyup for the reveal key (A).
 */
export function setAnnotationsRevealed(revealed: boolean): void {
  smartDiffState.annotationsRevealed = revealed;
}

/**
 * Set the active annotation (for keyboard navigation).
 */
export function setActiveAnnotation(id: string | null): void {
  smartDiffState.activeAnnotationId = id;
}

/**
 * Navigate to the next annotation in the current file.
 */
export function nextAnnotation(filePath: string): void {
  const result = smartDiffState.results.get(filePath);
  if (!result || result.annotations.length === 0) return;

  const currentIndex = result.annotations.findIndex(
    (a) => a.id === smartDiffState.activeAnnotationId
  );

  const nextIndex = currentIndex < result.annotations.length - 1 ? currentIndex + 1 : 0;
  smartDiffState.activeAnnotationId = result.annotations[nextIndex].id;
}

/**
 * Navigate to the previous annotation in the current file.
 */
export function prevAnnotation(filePath: string): void {
  const result = smartDiffState.results.get(filePath);
  if (!result || result.annotations.length === 0) return;

  const currentIndex = result.annotations.findIndex(
    (a) => a.id === smartDiffState.activeAnnotationId
  );

  const prevIndex = currentIndex > 0 ? currentIndex - 1 : result.annotations.length - 1;
  smartDiffState.activeAnnotationId = result.annotations[prevIndex].id;
}

// =============================================================================
// Persistence
// =============================================================================

/**
 * Load AI analysis results from the database for a diff.
 * Call this when switching to a new diff.
 */
export async function loadAnalysisFromDb(repoPath: string | null, spec: DiffSpec): Promise<void> {
  // Clear current in-memory state
  smartDiffState.results = new Map();
  smartDiffState.changesetSummary = null;
  smartDiffState.activeAnnotationId = null;

  try {
    // Load changeset summary
    const summary = await getChangesetSummary(repoPath, spec);
    if (summary) {
      smartDiffState.changesetSummary = summary;
    }

    // Load all file analyses
    const analyses = await getAllFileAnalyses(repoPath, spec);
    if (analyses.length > 0) {
      const newResults = new Map<string, SmartDiffResult>();
      for (const [path, result] of analyses) {
        newResults.set(path, result);
      }
      smartDiffState.results = newResults;
    }
  } catch (e) {
    console.error('Failed to load AI analysis from database:', e);
  }
}

/**
 * Delete all analyses from the database for a diff.
 * Used when refreshing analysis.
 */
export async function deleteAnalysis(repoPath: string | null, spec: DiffSpec): Promise<void> {
  try {
    await deleteAllAnalyses(repoPath, spec);
  } catch (e) {
    console.error('Failed to delete AI analysis from database:', e);
  }
}

// =============================================================================
// Cleanup
// =============================================================================

/**
 * Clear all analysis results from memory (e.g., when switching repos).
 * Does NOT delete from database - use deleteAnalysisFromDb for that.
 */
export function clearResults(): void {
  smartDiffState.results = new Map();
  smartDiffState.loading = false;
  smartDiffState.activeAnnotationId = null;
  smartDiffState.changesetSummary = null;
}

/**
 * Clear result for a specific file.
 */
export function clearFileResult(filePath: string): void {
  const newResults = new Map(smartDiffState.results);
  newResults.delete(filePath);
  smartDiffState.results = newResults;

  // Clear active annotation if it was in this file
  const result = smartDiffState.results.get(filePath);
  if (result?.annotations.some((a) => a.id === smartDiffState.activeAnnotationId)) {
    smartDiffState.activeAnnotationId = null;
  }
}

/**
 * Clear the analysis error (e.g., when user dismisses the error dialog).
 */
export function clearAnalysisError(): void {
  smartDiffState.analysisError = null;
}
