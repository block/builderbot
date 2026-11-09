import { describe, expect, it } from 'vitest';
import { normalizeAgentEnvVars, validateEnvVarName } from './agentEnvVars';

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

  it('drops an agent whose list is malformed without touching the others', () => {
    expect(
      normalizeAgentEnvVars({
        claude: { name: 'A', value: '1' },
        codex: [{ value: 'no name' }],
        goose: [{ name: 'A', value: '1', enabled: true }],
      })
    ).toEqual({ goose: [{ name: 'A', value: '1', enabled: true }] });
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
