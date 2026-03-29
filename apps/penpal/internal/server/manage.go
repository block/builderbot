package server

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/discovery"
	"github.com/loganj/penpal/internal/watcher"
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
// E-PENPAL-REMOVE-WORKSPACE: removes workspace, cleans up ProjectSources, refreshes.
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
	// Clean up any project source overrides for projects in this workspace
	for path := range s.cfg.ProjectSources {
		if filepath.Dir(path) == filepath.Clean(req.Path) {
			delete(s.cfg.ProjectSources, path)
		}
	}
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
// E-PENPAL-CLOSE-PROJECT: DELETE /api/projects removes standalone entry.
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

// handleAPISources dispatches source management requests.
func (s *Server) handleAPISources(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodPost:
		s.handleAddSource(w, r)
	case http.MethodDelete:
		s.handleRemoveSource(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

// handleAddSource handles POST /api/sources.
// Adds a file source (tree or individual file) to a project.
// The type is auto-detected: directories become "tree" sources,
// markdown files are added to a "files" source.
// E-PENPAL-ADD-SOURCE: POST /api/sources adds directory or file sources.
func (s *Server) handleAddSource(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Project string `json:"project"` // qualified name
		Path    string `json:"path"`    // relative to project root
		Name    string `json:"name"`    // display name (optional, for directories)
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}
	if req.Project == "" {
		http.Error(w, "project is required", http.StatusBadRequest)
		return
	}
	if req.Path == "" {
		http.Error(w, "path is required", http.StatusBadRequest)
		return
	}

	// Clean the path
	req.Path = filepath.Clean(req.Path)
	if filepath.IsAbs(req.Path) {
		http.Error(w, "path must be relative to project root", http.StatusBadRequest)
		return
	}

	// Find the project
	project := s.cache.FindProject(req.Project)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	// Check what the path points to
	absPath := filepath.Join(project.Path, req.Path)
	resolved, err := filepath.Abs(absPath)
	if err != nil || (resolved != filepath.Clean(project.Path) && !isSubpath(project.Path, resolved)) {
		http.Error(w, "invalid path", http.StatusBadRequest)
		return
	}
	absPath = resolved
	info, err := os.Stat(absPath)
	if err != nil {
		http.Error(w, fmt.Sprintf("path not found: %s", req.Path), http.StatusBadRequest)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	if info.IsDir() {
		// Directory → create a "tree" source
		sourceName := req.Name
		if sourceName == "" {
			sourceName = req.Path
		}

		// Check for duplicate source name
		for _, src := range project.Sources {
			if src.Name == sourceName {
				http.Error(w, fmt.Sprintf("source %q already exists", sourceName), http.StatusConflict)
				return
			}
		}

		newSource := config.SourceConfig{
			Type: "tree",
			Path: req.Path,
			Name: req.Name,
		}
		s.addSourceToConfig(project, newSource)
		s.refreshAfterConfigChange()
		log.Printf("Added tree source %q (%s) to project %s", sourceName, req.Path, req.Project)
	} else {
		// File → add to a "files" source
		if !strings.HasSuffix(req.Path, ".md") {
			http.Error(w, "only .md files can be added", http.StatusBadRequest)
			return
		}

		// Check if this file is already covered by an existing source
		for _, src := range project.Sources {
			// Skip the virtual __all_markdown__ source — it covers the whole
			// project tree but should not block manual file additions.
			if src.Name == "__all_markdown__" {
				continue
			}
			if src.Type == "thoughts" || src.Type == "tree" {
				if src.RootPath != "" && strings.HasPrefix(absPath, src.RootPath+"/") {
					http.Error(w, fmt.Sprintf("file is already included via source %q", src.Name), http.StatusConflict)
					return
				}
			}
			if src.Type == "files" {
				for _, f := range src.Files {
					if f == absPath {
						http.Error(w, "file is already included", http.StatusConflict)
						return
					}
				}
			}
		}

		// Find or create a "files" source in config
		added := s.addFileToConfig(project, req.Path)
		if !added {
			// Create a new files source
			newSource := config.SourceConfig{
				Type:  "files",
				Files: []string{req.Path},
			}
			s.addSourceToConfig(project, newSource)
		}
		s.refreshAfterConfigChange()
		log.Printf("Added file %s to project %s", req.Path, req.Project)
	}

	w.WriteHeader(http.StatusNoContent)
}

// addSourceToConfig appends a new source to the appropriate config location.
// Must be called with cfgMu held.
func (s *Server) addSourceToConfig(project *discovery.Project, src config.SourceConfig) {
	if project.Origin == "standalone" {
		for i, pc := range s.cfg.Projects {
			if filepath.Clean(pc.Path) == filepath.Clean(project.Path) {
				s.cfg.Projects[i].Sources = append(s.cfg.Projects[i].Sources, src)
				return
			}
		}
	} else {
		if s.cfg.ProjectSources == nil {
			s.cfg.ProjectSources = make(map[string][]config.SourceConfig)
		}
		s.cfg.ProjectSources[project.Path] = append(s.cfg.ProjectSources[project.Path], src)
	}
}

// addFileToConfig appends a file path to an existing "files" source in config.
// Returns true if an existing files source was found and updated.
// Must be called with cfgMu held.
func (s *Server) addFileToConfig(project *discovery.Project, relPath string) bool {
	if project.Origin == "standalone" {
		for i, pc := range s.cfg.Projects {
			if filepath.Clean(pc.Path) == filepath.Clean(project.Path) {
				for j, src := range pc.Sources {
					if src.Type == "files" {
						s.cfg.Projects[i].Sources[j].Files = append(s.cfg.Projects[i].Sources[j].Files, relPath)
						return true
					}
				}
				return false
			}
		}
	} else {
		if sources, ok := s.cfg.ProjectSources[project.Path]; ok {
			for j, src := range sources {
				if src.Type == "files" {
					s.cfg.ProjectSources[project.Path][j].Files = append(s.cfg.ProjectSources[project.Path][j].Files, relPath)
					return true
				}
			}
		}
	}
	return false
}

// handleRemoveSource handles DELETE /api/sources.
// Removes a user-added source or individual file from a project.
// Send {project, name} to remove a whole source (tree).
// Send {project, file} to remove an individual file from a "files" source.
// E-PENPAL-REMOVE-SOURCE: DELETE /api/sources removes user-added sources.
func (s *Server) handleRemoveSource(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Project string `json:"project"` // qualified name
		Name    string `json:"name"`    // source display name to remove (for tree sources)
		File    string `json:"file"`    // project-relative file path to remove (for individual files)
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid JSON: "+err.Error(), http.StatusBadRequest)
		return
	}
	if req.Project == "" {
		http.Error(w, "project is required", http.StatusBadRequest)
		return
	}
	if req.Name == "" && req.File == "" {
		http.Error(w, "name or file is required", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(req.Project)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	// Remove individual file from a "files" source
	if req.File != "" {
		s.cfgMu.Lock()
		defer s.cfgMu.Unlock()

		found := s.removeFileFromConfig(project, req.File)
		if !found {
			http.Error(w, "file not found in sources", http.StatusNotFound)
			return
		}
		s.refreshAfterConfigChange()
		log.Printf("Removed file %s from project %s", req.File, req.Project)
		w.WriteHeader(http.StatusNoContent)
		return
	}

	// Remove a whole source by name
	// Don't allow removing auto-detected sources
	for _, src := range project.Sources {
		if src.Name == req.Name && src.Auto {
			http.Error(w, "cannot remove auto-detected source", http.StatusBadRequest)
			return
		}
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	found := false
	if project.Origin == "standalone" {
		for i, pc := range s.cfg.Projects {
			if filepath.Clean(pc.Path) == filepath.Clean(project.Path) {
				var filtered []config.SourceConfig
				for _, src := range pc.Sources {
					effectiveName := src.Name
					if effectiveName == "" && src.Type == "tree" {
						effectiveName = src.Path
					}
					if effectiveName == req.Name {
						found = true
						continue
					}
					filtered = append(filtered, src)
				}
				s.cfg.Projects[i].Sources = filtered
				break
			}
		}
	} else {
		// Workspace project: remove from ProjectSources
		if sources, ok := s.cfg.ProjectSources[project.Path]; ok {
			var filtered []config.SourceConfig
			for _, src := range sources {
				effectiveName := src.Name
				if effectiveName == "" && src.Type == "tree" {
					effectiveName = src.Path
				}
				if effectiveName == req.Name {
					found = true
					continue
				}
				filtered = append(filtered, src)
			}
			if len(filtered) == 0 {
				delete(s.cfg.ProjectSources, project.Path)
			} else {
				s.cfg.ProjectSources[project.Path] = filtered
			}
		}
	}

	if !found {
		http.Error(w, "source not found", http.StatusNotFound)
		return
	}

	s.refreshAfterConfigChange()
	log.Printf("Removed source %q from project %s", req.Name, req.Project)
	w.WriteHeader(http.StatusNoContent)
}

// handleAPIOpen handles POST /api/open.
// Given a filesystem path, it resolves it to an existing project/file or adds
// it as a new standalone project/file source. Returns a JSON response with the
// URL to navigate to.
func (s *Server) handleAPIOpen(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		json.NewEncoder(w).Encode(map[string]string{"error": "invalid JSON: " + err.Error()})
		return
	}
	if req.Path == "" {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		json.NewEncoder(w).Encode(map[string]string{"error": "path is required"})
		return
	}

	absPath, err := filepath.Abs(expandTilde(req.Path))
	if err != nil {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		json.NewEncoder(w).Encode(map[string]string{"error": "invalid path: " + err.Error()})
		return
	}

	info, err := os.Stat(absPath)
	if err != nil {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		json.NewEncoder(w).Encode(map[string]string{"error": "path not found: " + absPath})
		return
	}

	w.Header().Set("Content-Type", "application/json")

	var url string
	if info.IsDir() {
		url = s.resolveOpenDirectory(w, absPath)
	} else {
		url = s.resolveOpenFile(w, absPath)
	}

	if url != "" {
		s.navMu.Lock()
		s.pendingNav = url
		s.pendingNavAt = time.Now()
		s.navMu.Unlock()
		s.watcher.Broadcast(watcher.Event{Type: watcher.EventNavigate, Path: url})
	}
}

// handleAPINavigate returns and clears any pending navigation from /api/open.
// The frontend calls this on SSE reconnect to catch navigations that fired
// while the connection was down (e.g., app was backgrounded).
func (s *Server) handleAPINavigate(w http.ResponseWriter, r *http.Request) {
	s.navMu.Lock()
	url := s.pendingNav
	at := s.pendingNavAt
	s.pendingNav = ""
	s.navMu.Unlock()

	w.Header().Set("Content-Type", "application/json")
	if url != "" && time.Since(at) < 30*time.Second {
		json.NewEncoder(w).Encode(map[string]string{"url": url})
	} else {
		json.NewEncoder(w).Encode(map[string]string{})
	}
}

// resolveOpenDirectory handles /api/open for a directory path.
// Checks if it matches an existing project, otherwise adds as standalone.
// Returns the resolved URL path.
func (s *Server) resolveOpenDirectory(w http.ResponseWriter, absPath string) string {
	// Check if this path is an existing project
	for _, p := range s.cache.Projects() {
		if filepath.Clean(p.Path) == filepath.Clean(absPath) {
			url := "/project/" + p.QualifiedName()
			json.NewEncoder(w).Encode(map[string]string{"url": url})
			return url
		}
	}

	// Check if already a standalone project in config
	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	for _, pc := range s.cfg.Projects {
		if filepath.Clean(pc.Path) == filepath.Clean(absPath) {
			// Already in config but maybe not discovered yet; refresh and return
			s.refreshAfterConfigChange()
			for _, p := range s.cache.Projects() {
				if filepath.Clean(p.Path) == filepath.Clean(absPath) {
					url := "/project/" + p.QualifiedName()
					json.NewEncoder(w).Encode(map[string]string{"url": url})
					return url
				}
			}
			json.NewEncoder(w).Encode(map[string]string{"url": "/"})
			return ""
		}
	}

	// Not found - add as a new standalone project
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{Path: absPath})
	s.refreshAfterConfigChange()

	// Find the newly added project
	for _, p := range s.cache.Projects() {
		if filepath.Clean(p.Path) == filepath.Clean(absPath) {
			log.Printf("Opened new standalone project: %s", absPath)
			url := "/project/" + p.QualifiedName()
			json.NewEncoder(w).Encode(map[string]string{"url": url})
			return url
		}
	}

	json.NewEncoder(w).Encode(map[string]string{"url": "/"})
	return ""
}

// resolveOpenFile handles /api/open for a file path.
// Checks if the file is in an existing project, otherwise adds it as a file
// source to the appropriate project or creates a new standalone project.
// Returns the resolved URL path.
func (s *Server) resolveOpenFile(w http.ResponseWriter, absPath string) string {
	// Find the best matching project using longest-prefix matching.
	// This ensures a file under a sub-project matches the sub-project
	// rather than a parent workspace root like "(root)".
	if p := s.cache.FindProjectByPath(absPath); p != nil {
		relPath, _ := filepath.Rel(p.Path, absPath)
		// Check if the file is in the cache (already being tracked)
		for _, f := range s.cache.ProjectFiles(p.QualifiedName()) {
			if f.FullPath == relPath {
				url := "/file/" + p.QualifiedName() + "/" + relPath
				json.NewEncoder(w).Encode(map[string]string{"url": url})
				return url
			}
		}
		// File is under a known project but not tracked.
		// If it's a .md file, add it as a file source.
		if strings.HasSuffix(absPath, ".md") {
			s.cfgMu.Lock()
			added := s.addFileToConfig(p, relPath)
			if !added {
				s.addSourceToConfig(p, config.SourceConfig{
					Type:  "files",
					Files: []string{relPath},
				})
			}
			s.refreshAfterConfigChange()
			s.cfgMu.Unlock()

			url := "/file/" + p.QualifiedName() + "/" + relPath
			log.Printf("Added file %s to project %s", relPath, p.QualifiedName())
			json.NewEncoder(w).Encode(map[string]string{"url": url})
			return url
		}
		// Non-markdown file in known project - just navigate to project
		url := "/project/" + p.QualifiedName()
		json.NewEncoder(w).Encode(map[string]string{"url": url})
		return url
	}

	// File is not under any known project. Create a standalone project from its
	// parent directory and add the file.
	if !strings.HasSuffix(absPath, ".md") {
		w.WriteHeader(http.StatusBadRequest)
		json.NewEncoder(w).Encode(map[string]string{"error": "only .md files can be opened directly"})
		return ""
	}

	projectDir := filepath.Dir(absPath)
	fileName := filepath.Base(absPath)

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	// Add as standalone project with this file as a source
	s.cfg.Projects = append(s.cfg.Projects, config.ProjectConfig{
		Path: projectDir,
		Sources: []config.SourceConfig{{
			Type:  "files",
			Files: []string{fileName},
		}},
	})
	s.refreshAfterConfigChange()

	// Find the new project and return its file URL
	for _, p := range s.cache.Projects() {
		if filepath.Clean(p.Path) == filepath.Clean(projectDir) {
			url := "/file/" + p.QualifiedName() + "/" + fileName
			log.Printf("Opened file %s in new project %s", fileName, p.QualifiedName())
			json.NewEncoder(w).Encode(map[string]string{"url": url})
			return url
		}
	}

	json.NewEncoder(w).Encode(map[string]string{"url": "/"})
	return ""
}

// removeFileFromConfig removes a single file from a "files" source in config.
// If the files source becomes empty, it is removed entirely.
// Must be called with cfgMu held.
func (s *Server) removeFileFromConfig(project *discovery.Project, relPath string) bool {
	if project.Origin == "standalone" {
		for i, pc := range s.cfg.Projects {
			if filepath.Clean(pc.Path) == filepath.Clean(project.Path) {
				for j, src := range pc.Sources {
					if src.Type == "files" {
						var filtered []string
						found := false
						for _, f := range src.Files {
							if f == relPath {
								found = true
								continue
							}
							filtered = append(filtered, f)
						}
						if found {
							if len(filtered) == 0 {
								// Remove the entire source entry
								s.cfg.Projects[i].Sources = append(pc.Sources[:j], pc.Sources[j+1:]...)
							} else {
								s.cfg.Projects[i].Sources[j].Files = filtered
							}
							return true
						}
					}
				}
				return false
			}
		}
	} else {
		if sources, ok := s.cfg.ProjectSources[project.Path]; ok {
			for j, src := range sources {
				if src.Type == "files" {
					var filtered []string
					found := false
					for _, f := range src.Files {
						if f == relPath {
							found = true
							continue
						}
						filtered = append(filtered, f)
					}
					if found {
						if len(filtered) == 0 {
							// Remove the entire source entry
							remaining := append(sources[:j], sources[j+1:]...)
							if len(remaining) == 0 {
								delete(s.cfg.ProjectSources, project.Path)
							} else {
								s.cfg.ProjectSources[project.Path] = remaining
							}
						} else {
							s.cfg.ProjectSources[project.Path][j].Files = filtered
						}
						return true
					}
				}
			}
		}
	}
	return false
}
