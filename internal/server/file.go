package server

import (
	"bytes"
	"html/template"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/loganj/birdseye/internal/discovery"
	"github.com/yuin/goldmark"
	highlighting "github.com/yuin/goldmark-highlighting/v2"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer/html"
)

var md = goldmark.New(
	goldmark.WithExtensions(
		extension.GFM,
		highlighting.NewHighlighting(
			highlighting.WithStyle("dracula"),
		),
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
	var project *discovery.Project
	for i := range s.projects {
		if s.projects[i].Name == projectName {
			project = &s.projects[i]
			break
		}
	}
	if project == nil {
		http.NotFound(w, r)
		return
	}

	fullPath := filepath.Join(project.ThoughtsPath, filePath)
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

	// Get parent directory path for back navigation
	parentPath := filepath.Dir(filePath)
	if parentPath == "." {
		parentPath = ""
	}

	data := struct {
		Project    *discovery.Project
		FilePath   string
		FileName   string
		ParentPath string
		Content    template.HTML
		Raw        string
	}{
		Project:    project,
		FilePath:   filePath,
		FileName:   filepath.Base(filePath),
		ParentPath: parentPath,
		Content:    template.HTML(buf.String()),
		Raw:        string(content),
	}
	s.tmpl.ExecuteTemplate(w, "file.html", data)
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
