import { describe, expect, it } from 'vitest';
import {
  DEFAULT_STRUCTURAL_HEADER_MAX_ROWS,
  getActiveStructuralStack,
  getHeaderAwareActiveStructuralStack,
  getStructuralDeclarations,
  type StructuralDeclaration,
} from './structuralHeaders';

function structuralDeclaration(
  name: string,
  lineIndex: number,
  endLineIndex: number,
  indent = 0
): StructuralDeclaration {
  return {
    lineIndex,
    indent,
    kind: 'function',
    name,
    displayText: name,
    endLineIndex,
  };
}

describe('structural headers', () => {
  it('activates declarations hidden behind rendered structural headers', () => {
    const declarations = [
      structuralDeclaration('Outer', 0, 30),
      structuralDeclaration('visibleParent', 5, 30, 2),
      structuralDeclaration('hiddenChild', 10, 20, 4),
    ];

    expect(getActiveStructuralStack(declarations, 8).map((d) => d.name)).toEqual([
      'Outer',
      'visibleParent',
    ]);
    expect(
      getHeaderAwareActiveStructuralStack(declarations, 8, DEFAULT_STRUCTURAL_HEADER_MAX_ROWS).map(
        (d) => d.name
      )
    ).toEqual(['Outer', 'visibleParent', 'hiddenChild']);
  });

  it('does not activate declarations below rendered structural headers', () => {
    const declarations = [
      structuralDeclaration('Outer', 0, 30),
      structuralDeclaration('visibleParent', 5, 30, 2),
      structuralDeclaration('nextChild', 11, 20, 4),
    ];

    expect(
      getHeaderAwareActiveStructuralStack(declarations, 8, DEFAULT_STRUCTURAL_HEADER_MAX_ROWS).map(
        (d) => d.name
      )
    ).toEqual(['Outer', 'visibleParent']);
  });

  it('caps covered rows to rendered structural header rows', () => {
    const declarations = [
      structuralDeclaration('Scope0', 0, 50),
      structuralDeclaration('Scope1', 1, 50, 2),
      structuralDeclaration('Scope2', 2, 50, 4),
      structuralDeclaration('Scope3', 3, 50, 6),
      structuralDeclaration('Scope4', 4, 50, 8),
      structuralDeclaration('Scope5', 5, 50, 10),
      structuralDeclaration('belowRenderedRows', 14, 30, 12),
    ];

    expect(getHeaderAwareActiveStructuralStack(declarations, 10, 3).map((d) => d.name)).toEqual([
      'Scope0',
      'Scope1',
      'Scope2',
      'Scope3',
      'Scope4',
      'Scope5',
    ]);
  });

  it('detects a TypeScript class with a nested method', () => {
    const lines = [
      'class Example {',
      '  static helloWorld() {',
      '    return "hello";',
      '  }',
      '}',
      'const outside = true;',
    ];

    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(declarations.map((declaration) => declaration.displayText)).toEqual([
      'class Example',
      'static helloWorld()',
    ]);
    expect(
      getActiveStructuralStack(declarations, 2).map((declaration) => declaration.name)
    ).toEqual(['Example', 'helloWorld']);
  });

  it('detects a Rust impl with a nested fn', () => {
    const lines = [
      'impl Example {',
      '    pub fn hello_world(&self) {',
      '        println!("hello");',
      '    }',
      '}',
      'fn outside() {}',
    ];

    const declarations = getStructuralDeclarations('example.rs', lines);

    expect(declarations.map((declaration) => declaration.kind)).toEqual([
      'impl',
      'function',
      'function',
    ]);
    expect(
      getActiveStructuralStack(declarations, 2).map((declaration) => declaration.displayText)
    ).toEqual(['impl Example', 'pub fn hello_world(&self)']);
  });

  it('detects a Swift class with a nested static func', () => {
    const lines = [
      'class Example {',
      '    static func helloWorld() {',
      '        print("hello")',
      '    }',
      '}',
    ];

    const declarations = getStructuralDeclarations('Example.swift', lines);

    expect(declarations.map((declaration) => declaration.displayText)).toEqual([
      'class Example',
      'static func helloWorld()',
    ]);
    expect(
      getActiveStructuralStack(declarations, 2).map((declaration) => declaration.name)
    ).toEqual(['Example', 'helloWorld']);
  });

  it('tracks Python indentation scopes', () => {
    const lines = [
      'class Example:',
      '    def hello_world(self):',
      '        return "hello"',
      '    def goodbye(self):',
      '        return "bye"',
      'print("done")',
    ];

    const declarations = getStructuralDeclarations('example.py', lines);

    expect(
      getActiveStructuralStack(declarations, 2).map((declaration) => declaration.name)
    ).toEqual(['Example', 'hello_world']);
    expect(
      getActiveStructuralStack(declarations, 3).map((declaration) => declaration.name)
    ).toEqual(['Example', 'goodbye']);
    expect(getActiveStructuralStack(declarations, 5)).toEqual([]);
  });

  it('returns no declarations for unsupported extensions', () => {
    expect(getStructuralDeclarations('README.md', ['# Example'])).toEqual([]);
  });

  it('ends brace scopes after the closing brace', () => {
    const lines = ['class Example {', '  method() {', '    return true;', '  }', '}', 'after();'];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 3).map((declaration) => declaration.name)
    ).toEqual(['Example', 'method']);
    expect(
      getActiveStructuralStack(declarations, 4).map((declaration) => declaration.name)
    ).toEqual(['Example']);
    expect(getActiveStructuralStack(declarations, 5)).toEqual([]);
  });

  it('does not extend body-less declarations into the next block', () => {
    const lines = [
      'interface Example {',
      '  helloWorld(): string;',
      '}',
      'function outside() {',
      '  return true;',
      '}',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 1).map((declaration) => declaration.name)
    ).toEqual(['Example']);
    expect(
      getActiveStructuralStack(declarations, 4).map((declaration) => declaration.name)
    ).toEqual(['outside']);
  });

  it('keeps a TypeScript function active past return type braces', () => {
    const lines = [
      'function findOpeningBrace(',
      '  lines: string[],',
      '  startLineIndex: number,',
      '  stopLineIndex: number',
      '): { lineIndex: number; columnIndex: number } | null {',
      '  const state = { inBlockComment: false };',
      '',
      '  for (let lineIndex = startLineIndex; lineIndex < stopLineIndex; lineIndex++) {',
      "    const columnIndex = lines[lineIndex].indexOf('{');",
      '    if (columnIndex !== -1) {',
      '      return { lineIndex, columnIndex };',
      '    }',
      '  }',
      '',
      '  return null;',
      '}',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 5).map((declaration) => declaration.name)
    ).toEqual(['findOpeningBrace']);
    expect(
      getActiveStructuralStack(declarations, 14).map((declaration) => declaration.name)
    ).toEqual(['findOpeningBrace']);
  });

  it('keeps a TypeScript function active past multiline destructured parameters', () => {
    const lines = [
      'export function useChatSessionController({',
      '  sessionId,',
      '  onMessageAccepted,',
      '}: UseChatSessionControllerOptions) {',
      '  return acceptMessage(sessionId, onMessageAccepted);',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('useChatSessionController.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 4).map((declaration) => declaration.name)
    ).toEqual(['useChatSessionController']);
    expect(getActiveStructuralStack(declarations, 6)).toEqual([]);
  });

  it('keeps a TypeScript function active past multiline parameters', () => {
    const lines = [
      'function formatMessage(',
      '  message: string,',
      '  repeatCount: number',
      ') {',
      '  return message.repeat(repeatCount);',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('formatMessage.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 4).map((declaration) => declaration.name)
    ).toEqual(['formatMessage']);
    expect(getActiveStructuralStack(declarations, 6)).toEqual([]);
  });

  it('keeps a TypeScript function active past multiline return type braces', () => {
    const lines = [
      'function loadUser(',
      '  id: string',
      '): {',
      '  user: User;',
      '  meta: { cached: boolean };',
      '} {',
      '  return { user: getUser(id), meta: { cached: false } };',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('loadUser.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 6).map((declaration) => declaration.name)
    ).toEqual(['loadUser']);
    expect(getActiveStructuralStack(declarations, 8)).toEqual([]);
  });

  it('keeps a TypeScript function active past single-line destructured parameters', () => {
    const lines = [
      'function selectUser({ id, name }: User) {',
      '  return { id, name };',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('selectUser.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 1).map((declaration) => declaration.name)
    ).toEqual(['selectUser']);
    expect(getActiveStructuralStack(declarations, 3)).toEqual([]);
  });

  it('ignores braces inside multiline TypeScript template strings', () => {
    const lines = [
      'function renderTemplate() {',
      '  const template = `',
      '    }',
      '  `;',
      '  return template;',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 4).map((declaration) => declaration.name)
    ).toEqual(['renderTemplate']);
    expect(getActiveStructuralStack(declarations, 6)).toEqual([]);
  });

  it('does not extend svelte script function scope into template braces', () => {
    const lines = [
      '<script lang="ts">',
      '  function paddingForIndent(indent: number): number {',
      '    return indent * 12;',
      '  }',
      '</script>',
      '',
      '<div>',
      '  {#if items.length > 0}',
      '    {#each items as item}',
      '      <span>{item.name}</span>',
      '    {/each}',
      '  {/if}',
      '</div>',
      '',
      '<style>',
      '  .container {',
      '    padding: 8px;',
      '  }',
      '</style>',
    ];
    const declarations = getStructuralDeclarations('Component.svelte', lines);

    expect(declarations).toHaveLength(1);
    expect(declarations[0].name).toBe('paddingForIndent');
    // Function body closes on line 3, so scope should end at line 4
    expect(
      getActiveStructuralStack(declarations, 2).map((declaration) => declaration.name)
    ).toEqual(['paddingForIndent']);
    expect(getActiveStructuralStack(declarations, 4)).toEqual([]);
    expect(getActiveStructuralStack(declarations, 7)).toEqual([]);
    expect(getActiveStructuralStack(declarations, 15)).toEqual([]);
  });

  it('scopes the last function in a file correctly when followed by unrelated code', () => {
    const lines = [
      'function first() {',
      '  return 1;',
      '}',
      '',
      'function last() {',
      '  return 2;',
      '}',
      '',
      '// trailing comment',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(declarations).toHaveLength(2);
    expect(
      getActiveStructuralStack(declarations, 5).map((declaration) => declaration.name)
    ).toEqual(['last']);
    expect(getActiveStructuralStack(declarations, 7)).toEqual([]);
  });

  it('handles multiple nested classes and methods', () => {
    const lines = [
      'class Outer {',
      '  method1() {',
      '    return 1;',
      '  }',
      '',
      '  method2() {',
      '    return 2;',
      '  }',
      '}',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(declarations.map((d) => d.name)).toEqual(['Outer', 'method1', 'method2']);
    expect(
      getActiveStructuralStack(declarations, 2).map((d) => d.name)
    ).toEqual(['Outer', 'method1']);
    expect(
      getActiveStructuralStack(declarations, 4).map((d) => d.name)
    ).toEqual(['Outer']);
    expect(
      getActiveStructuralStack(declarations, 6).map((d) => d.name)
    ).toEqual(['Outer', 'method2']);
    expect(getActiveStructuralStack(declarations, 9)).toEqual([]);
  });

  it('scopes Go receiver methods independently', () => {
    const lines = [
      'type Server struct {',
      '  port int',
      '}',
      '',
      'func (s *Server) Start() {',
      '  fmt.Println("starting")',
      '}',
      '',
      'func (s *Server) Stop() {',
      '  fmt.Println("stopping")',
      '}',
    ];
    const declarations = getStructuralDeclarations('server.go', lines);

    expect(declarations.map((d) => d.kind)).toEqual(['struct', 'method', 'method']);
    expect(
      getActiveStructuralStack(declarations, 5).map((d) => d.name)
    ).toEqual(['Start']);
    expect(
      getActiveStructuralStack(declarations, 9).map((d) => d.name)
    ).toEqual(['Stop']);
  });

  it('scopes Java class with methods', () => {
    const lines = [
      'public class Example {',
      '  public void run() {',
      '    System.out.println("running");',
      '  }',
      '',
      '  public int compute() {',
      '    return 42;',
      '  }',
      '}',
    ];
    const declarations = getStructuralDeclarations('Example.java', lines);

    expect(declarations.map((d) => d.name)).toEqual(['Example', 'run', 'compute']);
    expect(
      getActiveStructuralStack(declarations, 2).map((d) => d.name)
    ).toEqual(['Example', 'run']);
    expect(
      getActiveStructuralStack(declarations, 6).map((d) => d.name)
    ).toEqual(['Example', 'compute']);
  });

  it('ignores braces inside block comments', () => {
    const lines = [
      'function example() {',
      '  /* { */',
      '  return true;',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 2).map((d) => d.name)
    ).toEqual(['example']);
    expect(getActiveStructuralStack(declarations, 4)).toEqual([]);
  });

  it('ignores braces inside single-line strings', () => {
    const lines = [
      'function example() {',
      '  const s = "}";',
      '  return s;',
      '}',
      'after();',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(
      getActiveStructuralStack(declarations, 2).map((d) => d.name)
    ).toEqual(['example']);
    expect(getActiveStructuralStack(declarations, 4)).toEqual([]);
  });

  it('handles Kotlin fun declarations', () => {
    const lines = [
      'class Repository {',
      '  fun findById(id: Long) {',
      '    return db.find(id)',
      '  }',
      '}',
    ];
    const declarations = getStructuralDeclarations('Repository.kt', lines);

    expect(declarations.map((d) => d.name)).toEqual(['Repository', 'findById']);
    expect(
      getActiveStructuralStack(declarations, 2).map((d) => d.name)
    ).toEqual(['Repository', 'findById']);
  });

  it('handles Rust trait with methods', () => {
    const lines = [
      'pub trait Handler {',
      '    fn handle(&self) {',
      '        println!("default");',
      '    }',
      '}',
    ];
    const declarations = getStructuralDeclarations('handler.rs', lines);

    expect(declarations.map((d) => d.kind)).toEqual(['interface', 'function']);
    expect(
      getActiveStructuralStack(declarations, 2).map((d) => d.name)
    ).toEqual(['Handler', 'handle']);
  });

  it('handles deeply nested Python scopes', () => {
    const lines = [
      'class Outer:',
      '    class Inner:',
      '        def method(self):',
      '            return True',
      '    def other(self):',
      '        pass',
      'def top_level():',
      '    pass',
    ];
    const declarations = getStructuralDeclarations('example.py', lines);

    expect(
      getActiveStructuralStack(declarations, 3).map((d) => d.name)
    ).toEqual(['Outer', 'Inner', 'method']);
    expect(
      getActiveStructuralStack(declarations, 5).map((d) => d.name)
    ).toEqual(['Outer', 'other']);
    expect(
      getActiveStructuralStack(declarations, 7).map((d) => d.name)
    ).toEqual(['top_level']);
  });

  it('renders displayText correctly for destructured parameters', () => {
    const lines = [
      'function selectUser({ id, name }: User) {',
      '  return { id, name };',
      '}',
    ];
    const declarations = getStructuralDeclarations('selectUser.ts', lines);

    expect(declarations[0].displayText).toBe('function selectUser({ id, name }: User)');
  });

  it('renders displayText correctly for return type braces', () => {
    const lines = [
      'function findBrace(lines: string[]): { lineIndex: number } | null {',
      '  return null;',
      '}',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(declarations[0].displayText).toBe(
      'function findBrace(lines: string[]): { lineIndex: number } | null'
    );
  });

  it('does not match callback call expressions as method declarations', () => {
    const lines = [
      'class Example {',
      "  describe('test', () => {",
      '    return true;',
      '  });',
      '  onMount(() => {',
      '    setup();',
      '  });',
      '  $effect(() => {',
      '    update();',
      '  });',
      '  realMethod() {',
      '    return 1;',
      '  }',
      '}',
    ];
    const declarations = getStructuralDeclarations('Example.ts', lines);

    expect(declarations.map((d) => d.name)).toEqual(['Example', 'realMethod']);
  });
});
