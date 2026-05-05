import { describe, expect, it } from 'vitest';
import { getActiveStructuralStack, getStructuralDeclarations } from './structuralHeaders';

describe('structural headers', () => {
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
});
