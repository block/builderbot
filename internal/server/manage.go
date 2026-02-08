package server

import (
	"encoding/json"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/loganj/birdseye/internal/config"
	"github.com/loganj/birdseye/internal/watcher"
)

// expandTilde expands a leading ~ to the user's home directory.
func expandTilde(path string) string {
	if path == "~" || strings.HasPrefix(path, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, path[1:])
		}
	}
	return path
}

// refreshAfterConfigChange saves the config and re-discovers all projects.
func (s *Server) refreshAfterConfigChange() {
	if err := config.Save(s.cfgPath, s.cfg); err != nil {
		log.Printf("Warning: could not save config: %v", err)
	}
	projects := s.discoverAllProjects()
	s.cache.RescanWith(projects)
	s.watcher.Refresh(s.workspacePaths(), projects)
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventProjectsChanged})
	go s.populateProjects()
}

// handleAPIWorkspaces dispatches workspace management requests.
func (s *Server) handleAPIWorkspaces(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodPost:
		s.handleAddWorkspace(w, r)
	case http.MethodDelete:
		s.handleRemoveWorkspace(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// handleAddWorkspace handles POST /api/workspaces.
func (s *Server) handleAddWorkspace(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}
	if req.Path == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}

	absPath, err := filepath.Abs(expandTilde(req.Path))
	if err != nil {
		http.Error(w, "invalid path: "+err.Error(), http.StatusBadRequest)
		return
	}

	info, err := os.Stat(absPath)
	if err != nil || !info.IsDir() {
		http.Error(w, "path is not a directory", http.StatusBadRequest)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	// Check for duplicates
	for _, ws := range s.cfg.Workspaces {
		if filepath.Clean(ws.Path) == filepath.Clean(absPath) {
			http.Error(w, "workspace already exists", http.StatusConflict)
			return
		}
	}

	s.cfg.Workspaces = append(s.cfg.Workspaces, config.Workspace{Path: absPath})
	s.refreshAfterConfigChange()
	log.Printf("Added workspace: %s", absPath)
	w.WriteHeader(http.StatusNoContent)
}

// handleRemoveWorkspace handles DELETE /api/workspaces.
func (s *Server) handleRemoveWorkspace(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}
	if req.Path == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	found := false
	var filtered []config.Workspace
	for _, ws := range s.cfg.Workspaces {
		if filepath.Clean(ws.Path) == filepath.Clean(req.Path) {
			found = true
			continue
		}
		filtered = append(filtered, ws)
	}
	if !found {
		http.Error(w, "workspace not found", http.StatusNotFound)
		return
	}

	s.cfg.Workspaces = filtered
	s.refreshAfterConfigChange()
	log.Printf("Removed workspace: %s", req.Path)
	w.WriteHeader(http.StatusNoContent)
}

// handleAddStandaloneProject handles POST /api/projects.
func (s *Server) handleAddStandaloneProject(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}
	if req.Path == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}

	absPath, err := filepath.Abs(expandTilde(req.Path))
	if err != nil {
		http.Error(w, "invalid path: "+err.Error(), http.StatusBadRequest)
		return
	}

	info, err := os.Stat(absPath)
	if err != nil || !info.IsDir() {
		http.Error(w, "path is not a directory", http.StatusBadRequest)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	// Check not already a standalone project
	for _, pc := range s.cfg.Projects {
		if filepath.Clean(pc.Path) == filepath.Clean(absPath) {
			http.Error(w, "project already exists", http.StatusConflict)
			return
		}
	}

	// Check not already inside a workspace
	for _, ws := range s.cfg.Workspaces {
		if filepath.Dir(absPath) == filepath.Clean(ws.Path) {
			http.Error(w, "path is already within workspace "+ws.DisplayName(), http.StatusConflict)
			return
		}
	}

	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: absPath})
	s.refreshAfterConfigChange()
	log.Printf("Added standalone project: %s", absPath)
	w.WriteHeader(http.StatusNoContent)
}

// handleCloseStandaloneProject handles DELETE /api/projects.
// Removes the project from view without deleting any data.
func (s *Server) handleCloseStandaloneProject(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}
	if req.Path == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	found := false
	var filtered []config.ProjectConfig
	for _, pc := range s.cfg.Projects {
		if filepath.Clean(pc.Path) == filepath.Clean(req.Path) {
			found = true
			continue
		}
		filtered = append(filtered, pc)
	}
	if !found {
		http.Error(w, "project not found in config", http.StatusNotFound)
		return
	}

	s.cfg.Projects = filtered
	s.refreshAfterConfigChange()
	log.Printf("Closed standalone project: %s", req.Path)
	w.WriteHeader(http.StatusNoContent)
}
