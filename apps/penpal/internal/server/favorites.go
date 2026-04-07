package server

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/config"
	"github.com/loganj/penpal/internal/discovery"
	"github.com/loganj/penpal/internal/watcher"
)

type APIFavoriteEntry struct {
	ID    string    `json:"id"`
	Path  string    `json:"path"`
	Kind  string    `json:"kind"`
	Label string    `json:"label"`
	Files []APIFile `json:"files"`
}

func (s *Server) handleAPIFavorites(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		s.handleListFavorites(w, r)
	case http.MethodPost:
		s.handleAddFavorite(w, r)
	case http.MethodDelete:
		s.handleRemoveFavorite(w, r)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (s *Server) handleListFavorites(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	qualifiedName := r.URL.Query().Get("project")
	if qualifiedName == "" {
		http.Error(w, "project is required", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(qualifiedName)
	if project == nil {
		json.NewEncoder(w).Encode([]APIFavoriteEntry{})
		return
	}

	worktree := r.URL.Query().Get("worktree")
	cachedFiles := s.projectFilesForView(project, qualifiedName, worktree)
	json.NewEncoder(w).Encode(buildFavoriteEntries(project, cachedFiles))
}

func (s *Server) handleAddFavorite(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Project  string `json:"project"`
		Path     string `json:"path"`
		Worktree string `json:"worktree"`
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

	req.Path = filepath.Clean(req.Path)
	if filepath.IsAbs(req.Path) {
		http.Error(w, "path must be relative to project root", http.StatusBadRequest)
		return
	}

	project := s.cache.FindProject(req.Project)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	basePath := project.Path
	if req.Worktree != "" {
		basePath = s.cache.WorktreePath(req.Project, req.Worktree)
		if basePath == "" {
			http.Error(w, "worktree not found", http.StatusBadRequest)
			return
		}
	}

	absPath := filepath.Join(basePath, req.Path)
	resolved, err := filepath.Abs(absPath)
	if err != nil || (resolved != filepath.Clean(basePath) && !isSubpath(basePath, resolved)) {
		http.Error(w, "invalid path", http.StatusBadRequest)
		return
	}
	info, err := os.Stat(resolved)
	if err != nil {
		http.Error(w, fmt.Sprintf("path not found: %s", req.Path), http.StatusBadRequest)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	if info.IsDir() {
		if projectHasFavorite(project, req.Path, "tree") {
			http.Error(w, "favorite already exists", http.StatusConflict)
			return
		}
		s.addSourceToConfig(project, config.SourceConfig{Type: "tree", Path: req.Path})
		s.refreshAfterConfigChange()
		s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged, Project: req.Project})
		w.WriteHeader(http.StatusNoContent)
		return
	}

	if !strings.HasSuffix(req.Path, ".md") {
		http.Error(w, "only .md files can be favorited", http.StatusBadRequest)
		return
	}
	if projectHasFavorite(project, req.Path, "file") {
		http.Error(w, "favorite already exists", http.StatusConflict)
		return
	}
	added := s.addFileToConfig(project, req.Path)
	if !added {
		s.addSourceToConfig(project, config.SourceConfig{Type: "files", Files: []string{req.Path}})
	}
	s.refreshAfterConfigChange()
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged, Project: req.Project})
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleRemoveFavorite(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Project string `json:"project"`
		Path    string `json:"path"`
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

	req.Path = filepath.Clean(req.Path)
	project := s.cache.FindProject(req.Project)
	if project == nil {
		http.Error(w, "project not found", http.StatusNotFound)
		return
	}

	s.cfgMu.Lock()
	defer s.cfgMu.Unlock()

	removed := s.removeFavoriteFromConfig(project, req.Path)
	if !removed {
		http.Error(w, "favorite not found", http.StatusNotFound)
		return
	}

	s.refreshAfterConfigChange()
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged, Project: req.Project})
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) projectFilesForView(project *discovery.Project, qualifiedName, worktree string) []cache.FileInfo {
	if worktree != "" {
		wtPath := s.cache.WorktreePath(qualifiedName, worktree)
		if wtPath == "" {
			return nil
		}
		return cache.ScanProjectSourcesForWorktree(project, wtPath)
	}
	if s.cache.EnsureProjectScanned(qualifiedName) {
		for _, f := range s.cache.ProjectFiles(qualifiedName) {
			if f.Source != "__all_markdown__" {
				s.activity.RecordAt(activity.FileModified, f.Project, f.FullPath, f.ModTime)
			}
		}
	}
	return s.cache.ProjectFiles(qualifiedName)
}

func buildFavoriteEntries(project *discovery.Project, cachedFiles []cache.FileInfo) []APIFavoriteEntry {
	manualSources := make(map[string]bool)
	for _, src := range project.Sources {
		if src.SourceTypeName == "manual" {
			manualSources[src.Name] = true
		}
	}

	allMarkdown := make(map[string]cache.FileInfo)
	preferred := make(map[string]cache.FileInfo)
	for _, f := range cachedFiles {
		if existing, ok := preferred[f.FullPath]; !ok || preferredFavoriteFile(existing, f, manualSources) {
			preferred[f.FullPath] = f
		}
		if f.Source == "__all_markdown__" {
			allMarkdown[f.FullPath] = f
		}
	}

	entries := make([]APIFavoriteEntry, 0)
	seenEntries := make(map[string]bool)
	for _, src := range project.Sources {
		if src.SourceTypeName != "manual" {
			continue
		}
		if src.Type == "tree" {
			rootPath, err := filepath.Rel(project.Path, src.RootPath)
			if err != nil {
				continue
			}
			rootPath = filepath.Clean(rootPath)
			if rootPath == "." {
				rootPath = ""
			}
			entryID := "tree:" + rootPath
			if seenEntries[entryID] {
				continue
			}
			seenEntries[entryID] = true
			files := favoriteTreeFiles(rootPath, allMarkdown, preferred)
			entries = append(entries, APIFavoriteEntry{
				ID:    entryID,
				Path:  rootPath,
				Kind:  "tree",
				Label: favoriteLabel(rootPath),
				Files: files,
			})
			continue
		}
		if src.Type != "files" {
			continue
		}
		for _, absPath := range src.Files {
			relPath, err := filepath.Rel(project.Path, absPath)
			if err != nil {
				continue
			}
			relPath = filepath.Clean(relPath)
			entryID := "file:" + relPath
			if seenEntries[entryID] {
				continue
			}
			seenEntries[entryID] = true
			meta, ok := preferred[relPath]
			if !ok {
				meta, ok = allMarkdown[relPath]
			}
			if !ok {
				continue
			}
			entries = append(entries, APIFavoriteEntry{
				ID:    entryID,
				Path:  relPath,
				Kind:  "file",
				Label: favoriteLabel(relPath),
				Files: []APIFile{favoriteAPIFile(meta, relPath)},
			})
		}
	}

	return entries
}

func preferredFavoriteFile(existing cache.FileInfo, candidate cache.FileInfo, manualSources map[string]bool) bool {
	return favoriteFilePriority(candidate, manualSources) > favoriteFilePriority(existing, manualSources)
}

func favoriteFilePriority(info cache.FileInfo, manualSources map[string]bool) int {
	if manualSources[info.Source] {
		return 0
	}
	if info.Source == "__all_markdown__" {
		return 1
	}
	return 2
}

func favoriteTreeFiles(rootPath string, allMarkdown map[string]cache.FileInfo, preferred map[string]cache.FileInfo) []APIFile {
	paths := make([]string, 0)
	prefix := rootPath + string(filepath.Separator)
	for path := range allMarkdown {
		if rootPath == "" || path == rootPath || strings.HasPrefix(path, prefix) {
			paths = append(paths, path)
		}
	}
	sort.Strings(paths)

	files := make([]APIFile, 0, len(paths))
	for _, path := range paths {
		meta, ok := preferred[path]
		if !ok {
			meta = allMarkdown[path]
		}
		displayPath := path
		if rootPath != "" {
			displayPath = strings.TrimPrefix(path, prefix)
		}
		files = append(files, favoriteAPIFile(meta, displayPath))
	}
	return files
}

func favoriteLabel(relPath string) string {
	if relPath == "" {
		return "."
	}
	return relPath
}

func favoriteAPIFile(info cache.FileInfo, displayPath string) APIFile {
	return APIFile{
		Name:        info.Name,
		Title:       info.Title,
		Path:        info.FullPath,
		DisplayPath: displayPath,
		Source:      info.Source,
		SourceType:  info.SourceType,
		Age:         formatAge(info.ModTime),
		FileType:    info.FileType,
	}
}

func projectHasFavorite(project *discovery.Project, relPath, kind string) bool {
	relPath = filepath.Clean(relPath)
	for _, src := range project.Sources {
		if src.SourceTypeName != "manual" {
			continue
		}
		switch {
		case kind == "tree" && src.Type == "tree":
			rootPath, err := filepath.Rel(project.Path, src.RootPath)
			if err == nil && filepath.Clean(rootPath) == relPath {
				return true
			}
		case kind == "file" && src.Type == "files":
			for _, absPath := range src.Files {
				filePath, err := filepath.Rel(project.Path, absPath)
				if err == nil && filepath.Clean(filePath) == relPath {
					return true
				}
			}
		}
	}
	return false
}

func (s *Server) removeFavoriteFromConfig(project *discovery.Project, relPath string) bool {
	relPath = filepath.Clean(relPath)
	if project.Origin == "standalone" {
		for i, pc := range s.cfg.Projects {
			if filepath.Clean(pc.Path) != filepath.Clean(project.Path) {
				continue
			}
			filtered, removed := filterFavoriteConfigs(pc.Sources, relPath)
			if removed {
				s.cfg.Projects[i].Sources = filtered
			}
			return removed
		}
		return false
	}

	sources, ok := s.cfg.ProjectSources[project.Path]
	if !ok {
		return false
	}
	filtered, removed := filterFavoriteConfigs(sources, relPath)
	if !removed {
		return false
	}
	if len(filtered) == 0 {
		delete(s.cfg.ProjectSources, project.Path)
	} else {
		s.cfg.ProjectSources[project.Path] = filtered
	}
	return true
}

func filterFavoriteConfigs(sources []config.SourceConfig, relPath string) ([]config.SourceConfig, bool) {
	filtered := make([]config.SourceConfig, 0, len(sources))
	removed := false
	for _, src := range sources {
		switch src.Type {
		case "tree":
			if filepath.Clean(src.Path) == relPath {
				removed = true
				continue
			}
			filtered = append(filtered, src)
		case "files":
			nextFiles := make([]string, 0, len(src.Files))
			removedHere := false
			for _, file := range src.Files {
				if filepath.Clean(file) == relPath {
					removed = true
					removedHere = true
					continue
				}
				nextFiles = append(nextFiles, file)
			}
			if removedHere {
				if len(nextFiles) == 0 {
					continue
				}
				src.Files = nextFiles
			}
			filtered = append(filtered, src)
		default:
			filtered = append(filtered, src)
		}
	}
	return filtered, removed
}
