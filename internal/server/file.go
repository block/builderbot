package server

import (
	"bytes"
	"encoding/json"
	"html/template"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/discovery"
	"github.com/yuin/goldmark"
	highlighting "github.com/yuin/goldmark-highlighting/v2"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer/html"
)

type Heading struct {
	Level int
	ID    string
	Text  string
}

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
		html.WithUnsafe(), // Allow raw HTML in markdown
	),
)

func (s *Server) handleFile(w http.ResponseWriter, r *http.Request) {
	// Parse /file/{project}/{path}
	path := strings.TrimPrefix(r.URL.Path, "/file/")
	parts := strings.SplitN(path, "/", 2)
	if len(parts) < 2 {
		http.NotFound(w, r)
		return
	}
	projectName := parts[0]
	filePath := parts[1]

	// Find project
	project := s.cache.FindProject(projectName)
	if project == nil {
		http.NotFound(w, r)
		return
	}

	fullPath := filepath.Join(project.ThoughtsPath(), filePath)
	content, err := os.ReadFile(fullPath)
	if err != nil {
		http.NotFound(w, r)
		return
	}

	// Strip YAML frontmatter
	content = stripFrontmatter(content)

	var buf bytes.Buffer
	if err := md.Convert(content, &buf); err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	htmlContent := buf.String()
	headings := extractHeadings(htmlContent)

	// Get parent directory path for back navigation
	parentPath := filepath.Dir(filePath)
	if parentPath == "." {
		parentPath = ""
	}

	// Load comment threads for this file (comment store uses project-relative paths)
	threads, _ := s.comments.LoadThreads(projectName, "thoughts/"+filePath)
	anchorLines := comments.ResolveAnchorsToLines(threads, string(content))

	threadsJSON, _ := json.Marshal(threads)
	anchorLinesJSON, _ := json.Marshal(anchorLines)

	data := struct {
		Project     *discovery.Project
		FilePath    string
		FileName    string
		ParentPath  string
		FileType    string
		Content     template.HTML
		Headings    []Heading
		Raw         string
		ThreadsJSON template.JS
		AnchorLines template.JS
	}{
		Project:     project,
		FilePath:    filePath,
		FileName:    filepath.Base(filePath),
		ParentPath:  parentPath,
		FileType:    classifyFile(filePath),
		Content:     template.HTML(htmlContent),
		Headings:    headings,
		Raw:         string(content),
		ThreadsJSON: template.JS(threadsJSON),
		AnchorLines: template.JS(anchorLinesJSON),
	}
	s.getTemplate().ExecuteTemplate(w, "file.html", data)
}

func stripFrontmatter(content []byte) []byte {
	s := string(content)
	if !strings.HasPrefix(s, "---") {
		return content
	}

	// Find the closing ---
	rest := s[3:]
	idx := strings.Index(rest, "\n---")
	if idx == -1 {
		return content
	}

	// Return everything after the closing ---
	afterFrontmatter := rest[idx+4:]
	return []byte(strings.TrimLeft(afterFrontmatter, "\n"))
}

var headingRegex = regexp.MustCompile(`<h([1-3]) id="([^"]+)"[^>]*>([^<]+)</h[1-3]>`)

func extractHeadings(html string) []Heading {
	matches := headingRegex.FindAllStringSubmatch(html, -1)
	var headings []Heading
	for _, m := range matches {
		level := 1
		if m[1] == "2" {
			level = 2
		} else if m[1] == "3" {
			level = 3
		}
		headings = append(headings, Heading{
			Level: level,
			ID:    m[2],
			Text:  strings.TrimSpace(m[3]),
		})
	}
	return headings
}
