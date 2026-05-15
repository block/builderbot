import { describe, expect, it } from 'vitest';
import { formatToolDisplay, makePathsRelative } from './sessionModalHelpers';

describe('makePathsRelative', () => {
  it('matches symlink-style alias pairs', () => {
    expect(
      makePathsRelative('Read /Volumes/work/staged/repo/src/App.svelte', [
        '/Users/me/.staged/repo',
        '/Volumes/work/staged/repo',
      ])
    ).toBe('Read src/App.svelte');
  });

  it('uses the longest matching root before broader project roots', () => {
    expect(
      makePathsRelative(
        '/work/projects/p1/builderbot--feature/src/App.svelte /work/projects/p1/other--main/README.md',
        ['/work/projects/p1', '/work/projects/p1/builderbot--feature']
      )
    ).toBe('src/App.svelte other--main/README.md');
  });

  it('applies the ancestor fallback for subpath working directories', () => {
    expect(makePathsRelative('/work/repo/src/lib.rs', '/work/repo/apps/staged')).toBe('src/lib.rs');
  });

  it('formats shell command strings with resolved alias roots', () => {
    const call = JSON.stringify({
      name: 'Bash',
      arguments: {
        command:
          'sed -n "1,5p" /Volumes/work/repo/src/main.ts && cat /Users/me/link/repo/package.json',
      },
    });

    expect(formatToolDisplay(call, ['/Users/me/link/repo', '/Volumes/work/repo'], true)).toEqual({
      verb: 'Running',
      detail: 'sed -n "1,5p" src/main.ts && cat package.json',
    });
  });
});
