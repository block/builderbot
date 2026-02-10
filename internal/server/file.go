package server

import (
	"encoding/json"
	"html/template"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/loganj/birdseye/internal/comments"
	"github.com/loganj/birdseye/internal/discovery"
	"github.com/loganj/birdseye/internal/markdown"
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
	content = markdown.StripFrontmatter(content)

	htmlContent, err := markdown.Render(content)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}

	headings := markdown.ExtractHeadings(htmlContent)

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

	// Determine source info for this file from cache
	sourceType := ""
	fileType := classifyFile(filePath)
	cachedFiles := s.cache.ProjectFiles(project.QualifiedName())
	for _, cf := range cachedFiles {
		if cf.FullPath == filePath {
			sourceType = cf.SourceType
			fileType = cf.FileType
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
		Headings    []markdown.Heading
		Raw         string
		ThreadsJSON template.JS
		AnchorLines template.JS
	}{
		Project:     project,
		FilePath:    filePath,
		FileName:    filepath.Base(filePath),
		ParentPath:  parentPath,
		FileType:    fileType,
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

func (s *Server) handleRawFile(w http.ResponseWriter, r *http.Request) {
	qualifiedName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	if qualifiedName == "" || filePath == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	fullPath := filepath.Join(project.Path, filePath)
	content, err := os.ReadFile(fullPath)
	if err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	content = markdown.StripFrontmatter(content)
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Write(content)
}
