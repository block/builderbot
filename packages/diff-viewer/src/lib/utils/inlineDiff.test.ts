import { describe, expect, it } from 'vitest';
import {
  computeLineDiff,
  createLineDiffCache,
  type LineDiffResult,
  type CharHighlight,
} from './inlineDiff';

describe('computeLineDiff', () => {
  describe('identical lines', () => {
    it('marks all lines unchanged when before and after are identical', () => {
      const lines = ['line 1', 'line 2', 'line 3'];
      const result = computeLineDiff(lines, lines);

      expect(result.beforeLines).toEqual(['unchanged', 'unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'unchanged', 'unchanged']);
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('handles empty input', () => {
      const result = computeLineDiff([], []);
      expect(result.beforeLines).toEqual([]);
      expect(result.afterLines).toEqual([]);
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('handles single identical line', () => {
      const result = computeLineDiff(['hello'], ['hello']);
      expect(result.beforeLines).toEqual(['unchanged']);
      expect(result.afterLines).toEqual(['unchanged']);
    });
  });

  describe('pure additions and removals', () => {
    it('marks all lines as added when before is empty', () => {
      const result = computeLineDiff([], ['line 1', 'line 2']);
      expect(result.beforeLines).toEqual([]);
      expect(result.afterLines).toEqual(['added', 'added']);
    });

    it('marks all lines as removed when after is empty', () => {
      const result = computeLineDiff(['line 1', 'line 2'], []);
      expect(result.beforeLines).toEqual(['removed', 'removed']);
      expect(result.afterLines).toEqual([]);
    });

    it('marks completely different lines as removed/added', () => {
      const before = ['aaa xyz', 'bbb xyz'];
      const after = ['111 qqq', '222 qqq'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['removed', 'removed']);
      expect(result.afterLines).toEqual(['added', 'added']);
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('detects additions at the end with unchanged context', () => {
      const before = ['line 1', 'line 2'];
      const after = ['line 1', 'line 2', 'line 3'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'unchanged', 'added']);
    });

    it('detects removals at the beginning with unchanged context', () => {
      const before = ['line 0', 'line 1', 'line 2'];
      const after = ['line 1', 'line 2'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['removed', 'unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'unchanged']);
    });

    it('detects additions in the middle', () => {
      const before = ['line 1', 'line 3'];
      const after = ['line 1', 'line 2', 'line 3'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'added', 'unchanged']);
    });
  });

  describe('modified line detection', () => {
    it('pairs modified lines without offset', () => {
      const before = ['const x = 1;'];
      const after = ['const x = 2;'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['modified']);
      expect(result.afterLines).toEqual(['modified']);
      expect(result.modifiedPairs).toHaveLength(1);
      expect(result.modifiedPairs[0].beforeLineIndex).toBe(0);
      expect(result.modifiedPairs[0].afterLineIndex).toBe(0);
    });

    it('detects multiple modified pairs', () => {
      const before = ['const a = 1;', 'unchanged', 'const b = true;'];
      const after = ['const a = 2;', 'unchanged', 'const b = false;'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['modified', 'unchanged', 'modified']);
      expect(result.afterLines).toEqual(['modified', 'unchanged', 'modified']);
      expect(result.modifiedPairs).toHaveLength(2);
    });

    it('handles mixed modifications and unchanged lines', () => {
      const before = ['first', 'const x = 1;', 'middle', 'last'];
      const after = ['first', 'const x = 2;', 'middle', 'last'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'modified', 'unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'modified', 'unchanged', 'unchanged']);
      expect(result.modifiedPairs).toHaveLength(1);
    });
  });

  describe('insertion offset (peek-ahead)', () => {
    it('detects a modified pair when an insertion precedes it', () => {
      const before = ['  content: string | string[];'];
      const after = ['  newField: boolean;', '  content: string | string[];'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged']);
      expect(result.afterLines).toEqual(['added', 'unchanged']);
    });

    it('detects modification through an insertion offset', () => {
      const before = ['  pattern: string | string[];'];
      const after = ['  newField: boolean;', '  pattern: string[];'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['modified']);
      expect(result.afterLines).toEqual(['added', 'modified']);
      expect(result.modifiedPairs).toHaveLength(1);
      expect(result.modifiedPairs[0].beforeLineIndex).toBe(0);
      expect(result.modifiedPairs[0].afterLineIndex).toBe(1);
    });

    it('handles insertion before a modification with unchanged context', () => {
      const before = ['header', '  pattern: string | string[];', 'footer'];
      const after = ['header', '// totally new comment here', '  pattern: string[];', 'footer'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'modified', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'added', 'modified', 'unchanged']);
    });
  });

  describe('character highlights', () => {
    it('produces highlights for word-level changes', () => {
      const before = ['const x = 1;'];
      const after = ['const x = 2;'];
      const result = computeLineDiff(before, after);

      expect(result.modifiedPairs).toHaveLength(1);
      const pair = result.modifiedPairs[0];

      // "1" -> "2" are the changed tokens
      expect(pair.beforeHighlights.length).toBeGreaterThan(0);
      expect(pair.afterHighlights.length).toBeGreaterThan(0);
    });

    it('highlights only the changed word in a sentence', () => {
      const before = ['the quick brown fox'];
      const after = ['the slow brown fox'];
      const result = computeLineDiff(before, after);

      const pair = result.modifiedPairs[0];
      // "quick" is at position 4-9, "slow" is at position 4-8
      expect(pair.beforeHighlights).toEqual([{ start: 4, end: 9 }]);
      expect(pair.afterHighlights).toEqual([{ start: 4, end: 8 }]);
    });

    it('highlights multiple changed words', () => {
      const before = ['function foo(bar: string): number {'];
      const after = ['function baz(bar: boolean): string {'];
      const result = computeLineDiff(before, after);

      const pair = result.modifiedPairs[0];
      expect(pair.beforeHighlights.length).toBeGreaterThanOrEqual(2);
      expect(pair.afterHighlights.length).toBeGreaterThanOrEqual(2);
    });

    it('produces no highlights when lines are identical but in unmatched blocks', () => {
      // This shouldn't happen in practice since identical lines would be
      // caught by LCS, but the char-highlight logic should handle it
      const before = ['  return null;', '  return value;'];
      const after = ['  return value;'];
      const result = computeLineDiff(before, after);

      // "return null;" is removed, "return value;" is unchanged via LCS
      expect(result.beforeLines[0]).toBe('removed');
      expect(result.beforeLines[1]).toBe('unchanged');
      expect(result.afterLines[0]).toBe('unchanged');
    });
  });

  describe('complex scenarios', () => {
    it('handles a realistic code diff with mixed changes', () => {
      const before = [
        'import { foo } from "bar";',
        '',
        'export function hello() {',
        '  return "world";',
        '}',
      ];
      const after = [
        'import { foo, baz } from "bar";',
        'import { qux } from "quux";',
        '',
        'export function hello() {',
        '  return "universe";',
        '}',
      ];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines[0]).toBe('modified'); // import changed
      expect(result.beforeLines[1]).toBe('unchanged'); // empty line
      expect(result.beforeLines[2]).toBe('unchanged'); // function decl
      expect(result.beforeLines[3]).toBe('modified'); // return changed
      expect(result.beforeLines[4]).toBe('unchanged'); // closing brace

      expect(result.afterLines[0]).toBe('modified'); // import changed
      expect(result.afterLines[1]).toBe('added'); // new import
      expect(result.afterLines[2]).toBe('unchanged'); // empty line
      expect(result.afterLines[3]).toBe('unchanged'); // function decl
      expect(result.afterLines[4]).toBe('modified'); // return changed
      expect(result.afterLines[5]).toBe('unchanged'); // closing brace
    });

    it('handles multiple consecutive removals followed by additions', () => {
      const before = ['aaa', 'bbb', 'ccc'];
      const after = ['xxx', 'yyy', 'zzz'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['removed', 'removed', 'removed']);
      expect(result.afterLines).toEqual(['added', 'added', 'added']);
    });

    it('detects re-indented lines as modified within a hunk with context', () => {
      // Real-world case: code wrapped in an if-else block gains indentation.
      // The hunk alignment includes context lines (identical on both sides)
      // plus the changed lines. The re-indented lines should be "modified".
      const before = [
        'This is critical - the application parses this to link the PR.',
        '</action>"#,',
        '        pr_type = pr_type,',
        '        base_branch = base_branch,',
        '        branch_name = branch.branch_name,',
        '        draft_flag = draft_flag,',
        '    );',
        '',
        '    let mut session = store::Session::new_running(&prompt, &working_dir);',
        '    if let Some(ref p) = provider {',
      ];
      const after = [
        'This is critical - the application parses this to link the PR.',
        '</action>"#,',
        '            pr_type = pr_type,',
        '            base_branch = base_branch,',
        '            branch_name = branch.branch_name,',
        '            draft_flag = draft_flag,',
        '        )',
        '    };',
        '',
        '    let mut session = store::Session::new_running(&prompt, &working_dir);',
        '    if let Some(ref p) = provider {',
      ];
      const result = computeLineDiff(before, after);

      // Context lines should be unchanged
      expect(result.beforeLines[0]).toBe('unchanged');
      expect(result.beforeLines[1]).toBe('unchanged');
      expect(result.afterLines[0]).toBe('unchanged');
      expect(result.afterLines[1]).toBe('unchanged');

      // Re-indented lines should be modified, not removed/added
      expect(result.beforeLines[2]).toBe('modified'); // pr_type
      expect(result.beforeLines[3]).toBe('modified'); // base_branch
      expect(result.beforeLines[4]).toBe('modified'); // branch_name
      expect(result.beforeLines[5]).toBe('modified'); // draft_flag
      expect(result.beforeLines[6]).toBe('modified'); // ); -> )

      expect(result.afterLines[2]).toBe('modified'); // pr_type
      expect(result.afterLines[3]).toBe('modified'); // base_branch
      expect(result.afterLines[4]).toBe('modified'); // branch_name
      expect(result.afterLines[5]).toBe('modified'); // draft_flag
      expect(result.afterLines[6]).toBe('modified'); // )
      expect(result.afterLines[7]).toBe('added');    // };

      // Trailing context should be unchanged
      expect(result.beforeLines[7]).toBe('unchanged');
      expect(result.afterLines[8]).toBe('unchanged');
    });

    it('detects re-indented lines as modified when many insertions precede them', () => {
      // Edge case: if two hunks were merged into a single alignment, the
      // greedy matcher would see many inserted after-lines before reaching
      // the re-indented lines. The scan-ahead handles this.
      const before = [
        '    let prompt = format!(',
        '        r#"<action>',
        '        pr_type = pr_type,',
        '        base_branch = base_branch,',
        '        branch_name = branch.branch_name,',
        '        draft_flag = draft_flag,',
        '    );',
      ];
      const after = [
        '    let prompt = if let Some(ctx) = git_context {',
        '        format!(',
        '            r#"<action>',
        'Steps:',
        '1. Push the current branch to the remote',
        '</action>"#,',
        '            pr_type = pr_type,',
        '            base_branch = base_branch,',
        '            branch_name = branch.branch_name,',
        '            draft_flag = draft_flag,',
        '            log_output = ctx.log,',
        '            stat_output = ctx.stat,',
        '        )',
        '    } else {',
        '        format!(',
        '            r#"<action>',
        '            pr_type = pr_type,',
        '            base_branch = base_branch,',
        '            branch_name = branch.branch_name,',
        '            draft_flag = draft_flag,',
        '        )',
        '    };',
      ];
      const result = computeLineDiff(before, after);

      // The re-indented lines should be modified, not removed/added
      expect(result.beforeLines[2]).toBe('modified'); // pr_type
      expect(result.beforeLines[3]).toBe('modified'); // base_branch
      expect(result.beforeLines[4]).toBe('modified'); // branch_name
      expect(result.beforeLines[5]).toBe('modified'); // draft_flag
    });

    it('classifies completely replaced code blocks as removed/added, not modified', () => {
      // Real-world case: a `let prompt = format!(...)` block is replaced by
      // a `let git_context = pre_compute_git_context(...)` call preceded by
      // comments. The lines share structural tokens like `let ... = ...(` but
      // are semantically unrelated and should NOT be paired as modified.
      const before = [
        '    let prompt = format!(',
        '        r#"<action>',
        'Create a {pr_type} for the current branch.',
        '',
        'Steps:',
        '1. First, look at the diff between the current branch and when it branched off of the base branch `{base_branch}`.',
        '2. Push the current branch to the remote: `git push -u origin {branch_name}`',
        '3. Create a PR using the GitHub CLI: `gh pr create --base {base_branch} --fill-first{draft_flag}`',
      ];
      const after = [
        '    // Pre-compute git context in parallel so the agent can skip straight to',
        '    // pushing and creating the PR instead of running these deterministic',
        '    // commands itself.',
        '    let git_context = pre_compute_git_context(',
        '        is_remote,',
        '        &working_dir,',
        '        workspace_name.as_deref(),',
        '        &store,',
        '        &branch,',
        '        base_branch,',
        '    );',
      ];
      const result = computeLineDiff(before, after);

      // Every before-line should be removed, every after-line should be added
      for (let i = 0; i < before.length; i++) {
        expect(result.beforeLines[i]).toBe('removed');
      }
      for (let i = 0; i < after.length; i++) {
        expect(result.afterLines[i]).toBe('added');
      }
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('classifies replaced JSDoc descriptions as removed/added, not modified', () => {
      // Real-world case: a JSDoc comment is completely rewritten. The
      // description lines share structural tokens (` * `, `segments`,
      // `highlights`) which inflate similarity despite being semantically
      // unrelated. They should be removed/added, not modified.
      const before = [
        '/**',
        ' * Get highlighted token segments for a line, with search matches applied.',
        ' */',
      ];
      const after = [
        '/**',
        ' * Apply character-level diff highlights to segments by splitting them at highlight boundaries.',
        ' * Works similarly to applySearchHighlights — walks through segments tracking column position.',
        ' */',
      ];
      const result = computeLineDiff(before, after);

      // /** and */ are context
      expect(result.beforeLines[0]).toBe('unchanged');
      expect(result.beforeLines[2]).toBe('unchanged');
      expect(result.afterLines[0]).toBe('unchanged');
      expect(result.afterLines[3]).toBe('unchanged');

      // The description lines are too different to be "modified"
      expect(result.beforeLines[1]).toBe('removed');
      expect(result.afterLines[1]).toBe('added');
      expect(result.afterLines[2]).toBe('added');
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('highlights only the trailing comma when value is otherwise identical', () => {
      const before = ['    "dmg:publish": "node scripts/publish-dmg-to-github-release.mjs"'];
      const after = ['    "release:dmg:publish": "node scripts/publish-dmg-to-github-release.mjs",'];
      const result = computeLineDiff(before, after);

      expect(result.modifiedPairs).toHaveLength(1);
      const pair = result.modifiedPairs[0];

      // Only "release:" prefix and trailing "," should be highlighted on the after side,
      // NOT the entire value string.
      const afterHighlightedText = pair.afterHighlights.map(h =>
        after[0].slice(h.start, h.end),
      );
      expect(afterHighlightedText).toContain(',');
      expect(afterHighlightedText.join('')).toContain('release');

      // The shared value portion should NOT be highlighted
      const totalHighlighted = pair.afterHighlights.reduce((sum, h) => sum + (h.end - h.start), 0);
      expect(totalHighlighted).toBeLessThan(after[0].length / 2);
    });

    it('handles interleaved unchanged and changed lines', () => {
      const before = ['A', 'B', 'C', 'D', 'E'];
      const after = ['A', 'B2', 'C', 'D2', 'E'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines[0]).toBe('unchanged');
      expect(result.beforeLines[2]).toBe('unchanged');
      expect(result.beforeLines[4]).toBe('unchanged');
      // B and D are dissimilar enough to be removed/added rather than modified
      expect(result.afterLines[0]).toBe('unchanged');
      expect(result.afterLines[2]).toBe('unchanged');
      expect(result.afterLines[4]).toBe('unchanged');
    });
  });
});

describe('createLineDiffCache', () => {
  it('returns same result object for identical inputs', () => {
    const cache = createLineDiffCache();
    const before = ['const x = 1;'];
    const after = ['const x = 2;'];

    const result1 = cache.get(before, after);
    const result2 = cache.get(before, after);

    expect(result1).toBe(result2); // same reference
  });

  it('returns different result objects for different inputs', () => {
    const cache = createLineDiffCache();
    const result1 = cache.get(['a'], ['b']);
    const result2 = cache.get(['c'], ['d']);

    expect(result1).not.toBe(result2);
  });
});
