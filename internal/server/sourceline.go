package server

import (
	"strconv"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/text"
	"github.com/yuin/goldmark/util"
)

// sourceLineExtension adds data-source-line attributes to block-level elements.
type sourceLineExtension struct{}

func (e *sourceLineExtension) Extend(m goldmark.Markdown) {
	m.Parser().AddOptions(parser.WithASTTransformers(
		util.Prioritized(&sourceLineTransformer{}, 999),
	))
}

type sourceLineTransformer struct{}

func (t *sourceLineTransformer) Transform(node *ast.Document, reader text.Reader, pc parser.Context) {
	ast.Walk(node, func(n ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering {
			return ast.WalkContinue, nil
		}
		// Only add to block-level elements that have line info
		switch n.(type) {
		case *ast.Paragraph, *ast.Heading, *ast.ListItem, *ast.FencedCodeBlock, *ast.CodeBlock, *ast.Blockquote, *ast.HTMLBlock:
			if lines := n.Lines(); lines.Len() > 0 {
				line := lines.At(0)
				lineNum := countLines(reader.Source(), line.Start) + 1
				n.SetAttributeString("data-source-line", []byte(strconv.Itoa(lineNum)))
			} else if n.HasChildren() {
				// For containers like Blockquote/ListItem, use first child's line
				first := n.FirstChild()
				if first != nil {
					if fLines := first.Lines(); fLines.Len() > 0 {
						line := fLines.At(0)
						lineNum := countLines(reader.Source(), line.Start) + 1
						n.SetAttributeString("data-source-line", []byte(strconv.Itoa(lineNum)))
					}
				}
			}
		}
		return ast.WalkContinue, nil
	})
}

func countLines(source []byte, offset int) int {
	count := 0
	for i := 0; i < offset && i < len(source); i++ {
		if source[i] == '\n' {
			count++
		}
	}
	return count
}
