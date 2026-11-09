import { describe, expect, it } from 'vitest';
import {
  DUPLICATE_NAME_ERROR,
  LAUNCHER_RESERVED_NAMES,
  agentEnvVarRowErrors,
  agentEnvVarRowWarnings,
  duplicateEnabledNames,
  launcherReservedNameWarning,
  normalizeAgentEnvVars,
  validateEnvVar,
  validateEnvVarName,
  validateEnvVarValue,
  type AgentEnvVar,
} from './agentEnvVars';

function v(name: string, value = '', enabled = true): AgentEnvVar {
  return { name, value, enabled };
}

describe('normalizeAgentEnvVars', () => {
  it('reads a missing or malformed store value as no variables', () => {
    expect(normalizeAgentEnvVars(undefined)).toEqual({});
    expect(normalizeAgentEnvVars(null)).toEqual({});
    expect(normalizeAgentEnvVars('FOO=bar')).toEqual({});
    expect(normalizeAgentEnvVars([{ name: 'FOO', value: 'bar', enabled: true }])).toEqual({});
  });

  it('keeps each agent’s list in order and fills in defaults', () => {
    expect(
      normalizeAgentEnvVars({
        claude: [
          { name: 'B', value: '2', enabled: false },
          { name: 'A', value: '1', enabled: true },
          { name: 'LEGACY' },
        ],
        goose: [],
      })
    ).toEqual({
      claude: [
        { name: 'B', value: '2', enabled: false },
        { name: 'A', value: '1', enabled: true },
        { name: 'LEGACY', value: '', enabled: true },
      ],
      goose: [],
    });
  });

  it('drops an agent whose list is not a list without touching the others', () => {
    expect(
      normalizeAgentEnvVars({
        claude: { name: 'A', value: '1' },
        codex: 'FOO=bar',
        goose: [{ name: 'A', value: '1', enabled: true }],
      })
    ).toEqual({ goose: [{ name: 'A', value: '1', enabled: true }] });
  });

  it('skips only the malformed entries of a list and keeps the rest', () => {
    // A hand-edited file or an older shape must not cost the user the whole
    // list: `setAgentEnvVars` writes this record back, so a dropped list
    // would be erased from disk the next time any agent is edited.
    expect(
      normalizeAgentEnvVars({
        claude: [
          { name: 'KEEP_FIRST', value: '1', enabled: true },
          { value: 'no name' },
          null,
          'FOO=bar',
          ['NOT', 'AN', 'OBJECT'],
          { name: 42, value: 'numeric name' },
          { name: 'KEEP_LAST', value: 2, enabled: 'yes' },
        ],
      })
    ).toEqual({
      claude: [
        { name: 'KEEP_FIRST', value: '1', enabled: true },
        { name: 'KEEP_LAST', value: '', enabled: true },
      ],
    });
    expect(normalizeAgentEnvVars({ claude: [{ value: 'no name' }] })).toEqual({ claude: [] });
  });
});

describe('validateEnvVarName', () => {
  it('accepts usual variable names', () => {
    expect(validateEnvVarName('ANTHROPIC_API_KEY')).toBeNull();
    expect(validateEnvVarName('lower_case1')).toBeNull();
    expect(validateEnvVarName('1LEADING_DIGIT')).toBeNull();
  });

  it('explains an empty name, an =, whitespace or NUL', () => {
    expect(validateEnvVarName('')).toMatch(/name/i);
    expect(validateEnvVarName('A=B')).toMatch(/=/);
    expect(validateEnvVarName('A B')).toMatch(/spaces/);
    expect(validateEnvVarName('A\tB')).toMatch(/spaces/);
    expect(validateEnvVarName('A\0')).toMatch(/NUL/);
  });
});

describe('validateEnvVarValue', () => {
  it('accepts any text, including an empty value', () => {
    expect(validateEnvVarValue('')).toBeNull();
    expect(validateEnvVarValue('sk-ant-… with = and spaces')).toBeNull();
  });

  it('explains a NUL in the value', () => {
    expect(validateEnvVarValue('a\0b')).toMatch(/NUL/);
  });
});

describe('validateEnvVar', () => {
  it('reports the name before the value and nothing for a usable row', () => {
    expect(validateEnvVar(v('OK', 'fine'))).toBeNull();
    expect(validateEnvVar(v('A=B', 'a\0b'))).toMatch(/=/);
    expect(validateEnvVar(v('OK', 'a\0b'))).toMatch(/Values/);
  });
});

describe('duplicateEnabledNames', () => {
  it('names a variable set by two or more enabled rows', () => {
    expect(duplicateEnabledNames([v('A', '1'), v('B', '2'), v('A', '3')])).toEqual(new Set(['A']));
    expect(duplicateEnabledNames([v('A', '1'), v('A', '2'), v('A', '3')])).toEqual(new Set(['A']));
  });

  it('ignores disabled rows and rows the backend would skip', () => {
    expect(duplicateEnabledNames([v('A', '1'), v('A', '2', false)])).toEqual(new Set());
    expect(duplicateEnabledNames([v('A', '1'), v('A', 'a\0b')])).toEqual(new Set());
    expect(duplicateEnabledNames([v('', ''), v('', '')])).toEqual(new Set());
  });

  it('compares names exactly, like the backend', () => {
    expect(duplicateEnabledNames([v('path', '1'), v('PATH', '2')])).toEqual(new Set());
    expect(duplicateEnabledNames([v('A', '1'), v('A ', '2')])).toEqual(new Set());
  });
});

describe('agentEnvVarRowErrors', () => {
  it('leaves a freshly added blank row alone but flags a value with no name', () => {
    expect(agentEnvVarRowErrors([v('', '')])).toEqual([null]);
    expect(agentEnvVarRowErrors([v('', 'orphan')])).toEqual(['Enter a name']);
  });

  it('shows the inline error for every condition the backend skips a row on', () => {
    expect(
      agentEnvVarRowErrors([v('A=B', '1'), v('A B', '2'), v('A\0', '3'), v('OK', 'a\0b')])
    ).toEqual([
      'Names can’t contain =',
      'Names can’t contain spaces',
      'Names can’t contain NUL',
      'Values can’t contain NUL',
    ]);
  });

  it('marks every enabled row sharing a name, not only the later one', () => {
    expect(agentEnvVarRowErrors([v('A', '1'), v('B', '2'), v('A', '3')])).toEqual([
      DUPLICATE_NAME_ERROR,
      null,
      DUPLICATE_NAME_ERROR,
    ]);
  });

  it('does not count a disabled row as a duplicate or mark it', () => {
    expect(agentEnvVarRowErrors([v('A', '1'), v('A', '2', false)])).toEqual([null, null]);
    expect(agentEnvVarRowErrors([v('A', '1'), v('A', '2'), v('A', '3', false)])).toEqual([
      DUPLICATE_NAME_ERROR,
      DUPLICATE_NAME_ERROR,
      null,
    ]);
  });

  it('prefers a row’s own error over the duplicate warning', () => {
    expect(agentEnvVarRowErrors([v('A', '1'), v('A', '2'), v('A', 'x\0')])).toEqual([
      DUPLICATE_NAME_ERROR,
      DUPLICATE_NAME_ERROR,
      'Values can’t contain NUL',
    ]);
  });
});

describe('launcherReservedNameWarning', () => {
  it('warns on an enabled, valid row named after something the launcher sets', () => {
    for (const name of LAUNCHER_RESERVED_NAMES) {
      expect(launcherReservedNameWarning(v(name, '/opt/bin'))).toContain(name);
    }
    expect(launcherReservedNameWarning(v('PATH', ''))).toMatch(/Replaces the PATH/);
  });

  it('stays quiet for other names, disabled rows and rows the backend skips', () => {
    expect(launcherReservedNameWarning(v('ANTHROPIC_API_KEY', 'sk'))).toBeNull();
    expect(launcherReservedNameWarning(v('path', '/opt/bin'))).toBeNull();
    expect(launcherReservedNameWarning(v('PATH', '/opt/bin', false))).toBeNull();
    expect(launcherReservedNameWarning(v('PATH', 'a\0b'))).toBeNull();
  });
});

describe('agentEnvVarRowWarnings', () => {
  it('reports by position, independently of the row errors', () => {
    const entries = [
      v('PATH', '/a'),
      v('OK', '1'),
      v('GOOSE_SEARCH_PATHS', '[]', false),
      v('PATH', '/b'),
    ];
    expect(agentEnvVarRowWarnings(entries)).toEqual([
      'Replaces the PATH Staged sets for this agent',
      null,
      null,
      'Replaces the PATH Staged sets for this agent',
    ]);
    // Both PATH rows are also duplicates; the component shows the error and
    // hides the warning, but the helper stays position-for-position.
    expect(agentEnvVarRowErrors(entries)).toEqual([
      DUPLICATE_NAME_ERROR,
      null,
      null,
      DUPLICATE_NAME_ERROR,
    ]);
  });
});
