import { describe, expect, it } from 'vitest';
import { isSubpathSuggestionVisible } from './subpathSuggestions';

describe('isSubpathSuggestionVisible', () => {
  it('hides root dot directories until the user types a dot in the root segment', () => {
    expect(isSubpathSuggestionVisible('.github', '')).toBe(false);
    expect(isSubpathSuggestionVisible('.github', '.')).toBe(true);
    expect(isSubpathSuggestionVisible('.github', '.g')).toBe(true);
  });

  it('hides dot directories in child segments until that segment includes a dot', () => {
    expect(isSubpathSuggestionVisible('apps/.config', 'apps')).toBe(false);
    expect(isSubpathSuggestionVisible('apps/.config', 'apps/')).toBe(false);
    expect(isSubpathSuggestionVisible('apps/.config', 'apps/.')).toBe(true);
    expect(isSubpathSuggestionVisible('apps/.config', 'apps/.c')).toBe(true);
  });

  it('keeps descendants visible when an already typed parent segment starts with a dot', () => {
    expect(isSubpathSuggestionVisible('.github/workflows', '.github')).toBe(true);
    expect(isSubpathSuggestionVisible('.github/.actions', '.github')).toBe(false);
    expect(isSubpathSuggestionVisible('.github/.actions', '.github/.')).toBe(true);
  });

  it('still requires suggestions to match the typed prefix', () => {
    expect(isSubpathSuggestionVisible('packages/app', 'apps')).toBe(false);
    expect(isSubpathSuggestionVisible('apps/web', 'APP')).toBe(true);
  });
});
