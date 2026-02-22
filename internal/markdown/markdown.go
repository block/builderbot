package markdown

import (
	"bytes"
	"fmt"
	"regexp"
	"strings"

	"github.com/yuin/goldmark"
	highlighting "github.com/yuin/goldmark-highlighting/v2"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer/html"
	"github.com/yuin/goldmark/util"
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

// HeadingIDPrefix is prepended to all auto-generated heading IDs to avoid
// collisions with element IDs used by the application UI (e.g. "comments-panel").
const HeadingIDPrefix = "penpal-md-"

// Render converts markdown source to an HTML fragment string.
func Render(src []byte) (string, error) {
	var buf bytes.Buffer
	ctx := parser.NewContext(parser.WithIDs(&prefixedIDs{
		prefix: HeadingIDPrefix,
		used:   map[string]bool{},
	}))
	if err := md.Convert(src, &buf, parser.WithContext(ctx)); err != nil {
		return "", err
	}
	return buf.String(), nil
}

// prefixedIDs implements parser.IDs, prefixing every generated heading ID
// so that markdown content cannot collide with application element IDs.
type prefixedIDs struct {
	prefix string
	used   map[string]bool
}

func (p *prefixedIDs) Generate(value []byte, kind ast.NodeKind) []byte {
	value = util.TrimLeftSpace(value)
	value = util.TrimRightSpace(value)
	result := []byte(p.prefix)
	for i := 0; i < len(value); {
		v := value[i]
		l := util.UTF8Len(v)
		i += int(l)
		if l != 1 {
			continue
		}
		if util.IsAlphaNumeric(v) {
			if 'A' <= v && v <= 'Z' {
				v += 'a' - 'A'
			}
			result = append(result, v)
		} else if util.IsSpace(v) || v == '-' || v == '_' {
			result = append(result, '-')
		}
	}
	if len(result) == len(p.prefix) {
		if kind == ast.KindHeading {
			result = append(result, "heading"...)
		} else {
			result = append(result, "id"...)
		}
	}
	key := string(result)
	if !p.used[key] {
		p.used[key] = true
		return result
	}
	for i := 1; ; i++ {
		candidate := fmt.Sprintf("%s-%d", key, i)
		if !p.used[candidate] {
			p.used[candidate] = true
			return []byte(candidate)
		}
	}
}

func (p *prefixedIDs) Put(value []byte) {
	p.used[string(value)] = true
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
