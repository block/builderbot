export function normalizeSubpathInput(value: string): string {
  return value.trim().replace(/^\/+|\/+$/g, '');
}

// Split the value into parent path and current segment for suggestions.
// For "apps/on" -> parent="apps", segment="on"
// For "apps"    -> parent="",     segment="apps"
export function getSubpathParentPath(value: string): string {
  const trimmed = value.trim().replace(/^\/+/, '');
  const lastSlash = trimmed.lastIndexOf('/');
  if (lastSlash === -1) return '';
  return trimmed.substring(0, lastSlash);
}

export function getSubpathCurrentSegment(value: string): string {
  const trimmed = value.trim().replace(/^\/+/, '');
  const lastSlash = trimmed.lastIndexOf('/');
  if (lastSlash === -1) return trimmed;
  return trimmed.substring(lastSlash + 1);
}

export function isSubpathSuggestionVisible(suggestion: string, input: string): boolean {
  const normalizedInput = normalizeSubpathInput(input);
  const normalizedSuggestion = normalizeSubpathInput(suggestion);
  const lowerInput = normalizedInput.toLowerCase();

  if (lowerInput && !normalizedSuggestion.toLowerCase().startsWith(lowerInput)) {
    return false;
  }

  const inputSegments = normalizedInput.split('/');

  return normalizedSuggestion.split('/').every((segment, index) => {
    if (!segment.startsWith('.')) return true;
    return inputSegments[index]?.includes('.') ?? false;
  });
}
