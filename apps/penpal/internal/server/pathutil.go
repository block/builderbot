package server

import (
	"path/filepath"
	"strings"
)

// isSubpath reports whether child is strictly inside parent after
// cleaning both paths. It prevents path-traversal attacks by ensuring
// the resolved child starts with the parent directory prefix.
// Returns false when child equals parent (e.g. path=".").
func isSubpath(parent, child string) bool {
	parent = filepath.Clean(parent)
	child = filepath.Clean(child)
	if parent == child {
		return false
	}
	return strings.HasPrefix(child, parent+string(filepath.Separator))
}
