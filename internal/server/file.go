package server

import (
	"bytes"
	"encoding/json"
	"html/template"
	"log"
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
	// Parse /file/{qualifiedName}/{filePath}
	// qualifiedName may be "workspace/project" (2 segments) or "project" (1 segment).
	// Try 2-segment match first, then fall back to 1-segment.
	rest := strings.TrimPrefix(r.URL.Path, "/file/")

	var project *discovery.Project
	var filePath string

	// Try 2-segment qualified name: workspace/project/path
	parts := strings.SplitN(rest, "/", 3)
	if len(parts) >= 3 {
		qn := parts[0] + "/" + parts[1]
		if p := s.cache.FindProject(qn); p != nil {
			project = p
			filePath = parts[2]
		}
	}
	// Fall back to 1-segment qualified name: project/path
	if project == nil && len(parts) >= 2 {
		if p := s.cache.FindProject(parts[0]); p != nil {
			project = p
			filePath = strings.Join(parts[1:], "/")
		}
	}
	if project == nil || filePath == "" {
		http.NotFound(w, r)
		return
	}

	fullPath := filepath.Join(project.Path, filePath)
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

	// Load comment threads for this file (paths are project-relative)
	threads, _ := s.comments.LoadThreads(project.QualifiedName(), filePath)
	anchorLines := comments.ResolveAnchorsToLines(threads, string(content))

	// Wrap threads with typing state for initial render
	qualName := project.QualifiedName()
	type threadWithTyping struct {
		comments.Thread
		AgentTyping bool `json:"agentTyping,omitempty"`
	}
	wrapped := make([]threadWithTyping, len(threads))
	for i, t := range threads {
		wrapped[i] = threadWithTyping{Thread: t}
		if s.comments.IsTyping(qualName, filePath, t.ID) {
			wrapped[i].AgentTyping = true
		}
	}
	threadsJSON, _ := json.Marshal(wrapped)
	anchorLinesJSON, _ := json.Marshal(anchorLines)

	// Determine source type for this file (needed for menu options)
	sourceType := ""
	cachedFiles := s.cache.ProjectFiles(project.QualifiedName())
	for _, cf := range cachedFiles {
		if cf.FullPath == filePath {
			sourceType = cf.SourceType
			break
		}
	}

	data := struct {
		Project     *discovery.Project
		FilePath    string
		FileName    string
		ParentPath  string
		FileType    string
		SourceType  string
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
		SourceType:  sourceType,
		Content:     template.HTML(htmlContent),
		Headings:    headings,
		Raw:         string(content),
		ThreadsJSON: template.JS(threadsJSON),
		AnchorLines: template.JS(anchorLinesJSON),
	}
	nav := s.buildNav(project.QualifiedName())
	nav.InProject = true
	s.renderPage(w, "file.html", nav, data)

	// Auto-start agent if there are pending human comments and no agent running
	if s.agents != nil && s.agents.Status(qualName) == nil {
		if s.comments.HasPendingHumanComments(qualName) {
			go func() {
				if _, err := s.agents.Start(qualName); err != nil {
					log.Printf("Auto-start agent on view for %s: %v", qualName, err)
				}
			}()
		}
	}
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
