import { visit } from 'unist-util-visit';
import type { Root } from 'mdast';

/**
 * Block-level mdast node types that can serve as anchoring targets for comments.
 * Maps to the same elements as the Go goldmark sourceLineTransformer.
 */
const BLOCK_TYPES = new Set([
  'paragraph', 'heading', 'listItem', 'code',
  'blockquote', 'html', 'table', 'thematicBreak',
]);

/**
 * Remark plugin that sets data-source-line on block-level elements from their
 * AST position. Runs at the mdast level (before rehypeRaw), so positions are
 * always available — matching the Go goldmark sourceLineTransformer approach.
 *
 * Uses node.data.hProperties which remark-rehype carries through to the
 * resulting hast elements automatically.
 */
// E-PENPAL-MD-RENDER: sets data-source-line on block-level elements from AST positions.
export default function remarkSourceLine() {
  return (tree: Root) => {
    visit(tree, (node) => {
      if (!BLOCK_TYPES.has(node.type)) return;
      const line = node.position?.start?.line;
      if (!line) return;
      const data: Record<string, unknown> = ((node as any).data ??= {});
      const hProps: Record<string, unknown> = (data.hProperties ??= {}) as Record<string, unknown>;
      hProps['dataSourceLine'] = line;
    });
  };
}
