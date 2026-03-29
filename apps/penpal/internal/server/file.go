package server

import (
	"net/http"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/markdown"
)

// E-PENPAL-FRONTMATTER-STRIP: markdown.StripFrontmatter() applied in handleRawFile.
// E-PENPAL-PATH-TRAVERSAL: resolveProjectFile() prevents path traversal on raw files.
func (s *Server) handleRawFile(w http.ResponseWriter, r *http.Request) {
	qualifiedName := r.URL.Query().Get("project")
	filePath := r.URL.Query().Get("path")
	worktree := r.URL.Query().Get("worktree")
	if qualifiedName == "" || filePath == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}

	content, ok := s.resolveProjectFile(qualifiedName, filePath, worktree)
	if !ok {
		http.Error(w, "file not found", http.StatusNotFound)
		return
	}

	content = markdown.StripFrontmatter(content)
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.Write(content)
}

// handleRecordView records a file-viewed activity event.
// E-PENPAL-ACTIVITY: records file-viewed events.
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
