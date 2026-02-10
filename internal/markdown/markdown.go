package markdown

import (
	"bytes"
	"regexp"
	"strings"

	"github.com/yuin/goldmark"
	highlighting "github.com/yuin/goldmark-highlighting/v2"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer/html"
)

// Heading represents a heading extracted from rendered HTML.
type Heading struct {
	Level int
	ID    string
	Text  string
}

// md is the shared goldmark pipeline used by both the file handler and the publisher.
var md = goldmark.New(
	goldmark.WithExtensions(
		extension.GFM,
		highlighting.NewHighlighting(
			highlighting.WithStyle("dracula"),
		),
		&sourceLineExtension{},
	),
	goldmark.WithParserOptions(
		parser.WithAutoHeadingID(),
	),
	goldmark.WithRendererOptions(
		html.WithUnsafe(),
	),
)

// Render converts markdown source to an HTML fragment string.
func Render(src []byte) (string, error) {
	var buf bytes.Buffer
	if err := md.Convert(src, &buf); err != nil {
		return "", err
	}
	return buf.String(), nil
}

var headingRegex = regexp.MustCompile(`<h([1-3]) id="([^"]+)"[^>]*>(.*?)</h[1-3]>`)
var htmlTagRegex = regexp.MustCompile(`<[^>]+>`)

// ExtractHeadings parses rendered HTML to find h1-h3 elements with IDs.
func ExtractHeadings(htmlStr string) []Heading {
	matches := headingRegex.FindAllStringSubmatch(htmlStr, -1)
	var headings []Heading
	for _, m := range matches {
		level := 1
		if m[1] == "2" {
			level = 2
		} else if m[1] == "3" {
			level = 3
		}
		text := htmlTagRegex.ReplaceAllString(m[3], "")
		headings = append(headings, Heading{
			Level: level,
			ID:    m[2],
			Text:  strings.TrimSpace(text),
		})
	}
	return headings
}

// StripFrontmatter removes YAML frontmatter delimited by --- from the beginning of content.
func StripFrontmatter(content []byte) []byte {
	s := string(content)
	if !strings.HasPrefix(s, "---") {
		return content
	}
	rest := s[3:]
	idx := strings.Index(rest, "\n---")
	if idx == -1 {
		return content
	}
	afterFrontmatter := rest[idx+4:]
	return []byte(strings.TrimLeft(afterFrontmatter, "\n"))
}
