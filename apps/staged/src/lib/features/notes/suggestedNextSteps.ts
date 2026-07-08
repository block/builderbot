import type { SuggestedNextStep } from '../../types';

interface SuggestedNextStepSource {
  suggestedNextSteps?: SuggestedNextStep[] | null;
  suggestedNextCommitStep?: string | null;
  suggestedNextNoteStep?: string | null;
}

export function suggestedNextStepsForNote(source: SuggestedNextStepSource): SuggestedNextStep[] {
  const typedSteps = (source.suggestedNextSteps ?? []).filter((step) => step.prompt.trim());
  if (typedSteps.length > 0) return typedSteps;

  const steps: SuggestedNextStep[] = [];
  if (source.suggestedNextCommitStep?.trim()) {
    steps.push({
      type: 'implementation',
      prompt: source.suggestedNextCommitStep.trim(),
      expectedMultipleCommits: false,
    });
  }
  if (source.suggestedNextNoteStep?.trim()) {
    steps.push({
      type: 'note',
      prompt: source.suggestedNextNoteStep.trim(),
    });
  }
  return steps;
}

export function suggestedNextStepButtonLabel(step: SuggestedNextStep): string {
  if (step.type === 'note') return 'Start note';
  return step.expectedMultipleCommits ? 'Start series' : 'Start commit';
}
