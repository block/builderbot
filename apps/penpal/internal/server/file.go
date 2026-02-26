package server

import (
	"net/http"
	"os"
	"path/filepath"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/markdown"
)

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

	// Prevent path traversal: resolved path must stay within the project root.
	resolved, err := filepath.Abs(fullPath)
	if err != nil || !isSubpath(project.Path, resolved) {
		http.Error(w, "invalid path", http.StatusBadRequest)
		return
	}

	content, err := os.ReadFile(resolved)
	if err != nil {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	content = markdown.StripFrontmatter(content)
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.Write(content)
}

// handleRecordView records a file-viewed activity event.
func (s *Server) handleRecordView(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	qualifiedName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	if qualifiedName == "" || filePath == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}
	s.activity.Record(activity.FileViewed, qualifiedName, filePath)
	w.WriteHeader(http.StatusNoContent)
}
