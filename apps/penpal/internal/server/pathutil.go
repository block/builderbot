package server

import (
	"path/filepath"
	"strings"
)

// isSubpath reports whether child is inside (or equal to) parent after
// cleaning both paths. It prevents path-traversal attacks by ensuring
// the resolved child starts with the parent directory prefix.
func isSubpath(parent, child string) bool {
	parent = filepath.Clean(parent) + string(filepath.Separator)
	child = filepath.Clean(child) + string(filepath.Separator)
	return strings.HasPrefix(child, parent)
}
