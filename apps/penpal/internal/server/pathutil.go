package server

import (
	"os"
	"path/filepath"
	"strings"
)

// isSubpath reports whether child is strictly inside parent after
// cleaning both paths. It prevents path-traversal attacks by ensuring
// the resolved child starts with the parent directory prefix.
// Returns false when child equals parent (e.g. path=".").
// E-PENPAL-PATH-TRAVERSAL: isSubpath() prevents path traversal on comments, raw files, source-add.
func isSubpath(parent, child string) bool {
	parent = filepath.Clean(parent)
	child = filepath.Clean(child)
	if parent == child {
		return false
	}
	return strings.HasPrefix(child, parent+string(filepath.Separator))
}

// resolveProjectFile resolves a project-relative file path to an absolute
// path with path-traversal protection. It returns the file contents and
// true on success, or nil and false if the project/worktree is not found,
// the path escapes the base directory, or the file cannot be read.
// E-PENPAL-PATH-TRAVERSAL: shared traversal check for file and threads endpoints.
func (s *Server) resolveProjectFile(projectName, filePath, worktree string) ([]byte, bool) {
	project := s.cache.FindProject(projectName)
	if project == nil {
		return nil, false
	}
	basePath := project.Path
	if worktree != "" {
		if wtPath := s.cache.WorktreePath(projectName, worktree); wtPath != "" {
			basePath = wtPath
		}
	}
	fullPath := filepath.Join(basePath, filePath)
	resolved, err := filepath.Abs(fullPath)
	if err != nil || !isSubpath(basePath, resolved) {
		return nil, false
	}
	raw, err := os.ReadFile(resolved)
	if err != nil {
		return nil, false
	}
	return raw, true
}
