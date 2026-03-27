package publish

import (
	"strings"
	"testing"
)

// E-PENPAL-PUBLISH-RENDER: verifies RenderHTML produces valid HTML with TOC and content.
func TestRenderHTML_BasicStructure(t *testing.T) {
	md := []byte("# Hello World\n\nSome paragraph text.\n\n## Section Two\n\nMore text here.\n")

	out, err := RenderHTML(md, "Test Page")
	if err != nil {
		t.Fatalf("RenderHTML failed: %v", err)
	}
	html := string(out)

	checks := []struct {
		name   string
		substr string
	}{
		{"doctype", "<!DOCTYPE html>"},
		{"title tag", "<title>Test Page</title>"},
		{"article class", `<article class="content">`},
		{"h1 rendered", "Hello World"},
		{"h2 rendered", "Section Two"},
		{"paragraph", "Some paragraph text."},
		{"toc h1 link", `href="#penpal-md-hello-world"`},
		{"toc h2 link", `href="#penpal-md-section-two"`},
		{"toc level class", `class="level-2"`},
		{"mermaid script", "mermaid.initialize"},
		{"copy button", `class="copy-md-btn"`},
		{"raw markdown template", `<template id="raw-markdown">`},
	}

	for _, c := range checks {
		if !strings.Contains(html, c.substr) {
			t.Errorf("%s: expected HTML to contain %q", c.name, c.substr)
		}
	}
}

// E-PENPAL-PUBLISH-RENDER: verifies raw markdown is embedded in the HTML page.
func TestRenderHTML_RawMarkdownEmbedded(t *testing.T) {
	md := []byte("# Title\n\nSome **bold** text.\n")

	out, err := RenderHTML(md, "Raw Test")
	if err != nil {
		t.Fatalf("RenderHTML failed: %v", err)
	}
	html := string(out)

	// The raw markdown should be HTML-escaped inside the template tag
	if !strings.Contains(html, "Some **bold** text.") {
		t.Error("expected raw markdown to be embedded in page")
	}
}

// E-PENPAL-PUBLISH-RENDER: verifies frontmatter is stripped before rendering.
func TestRenderHTML_StripsFrontmatter(t *testing.T) {
	md := []byte("---\ntitle: My Doc\ntags: [foo]\n---\n\n# Actual Content\n\nBody text.\n")

	out, err := RenderHTML(md, "FM Test")
	if err != nil {
		t.Fatalf("RenderHTML failed: %v", err)
	}
	html := string(out)

	if strings.Contains(html, "tags: [foo]") {
		t.Error("frontmatter was not stripped")
	}
	if !strings.Contains(html, "Actual Content") {
		t.Error("content after frontmatter missing")
	}
}

// E-PENPAL-PUBLISH-RENDER: verifies syntax highlighting is applied to code blocks.
func TestRenderHTML_SyntaxHighlighting(t *testing.T) {
	md := []byte("# Code\n\n```go\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n```\n")

	out, err := RenderHTML(md, "Code Test")
	if err != nil {
		t.Fatalf("RenderHTML failed: %v", err)
	}
	html := string(out)

	// Dracula theme uses class="chroma" for syntax highlighted blocks
	if !strings.Contains(html, "chroma") {
		t.Error("expected syntax highlighting (chroma class) in code block")
	}
}

// E-PENPAL-PUBLISH-RENDER: verifies markdown tables are rendered to HTML.
func TestRenderHTML_Table(t *testing.T) {
	md := []byte("# Tables\n\n| Name | Value |\n|------|-------|\n| foo  | bar   |\n")

	out, err := RenderHTML(md, "Table Test")
	if err != nil {
		t.Fatalf("RenderHTML failed: %v", err)
	}
	html := string(out)

	if !strings.Contains(html, "<table>") {
		t.Error("expected table element")
	}
	if !strings.Contains(html, "<th>") {
		t.Error("expected table header")
	}
}
