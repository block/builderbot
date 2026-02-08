package agents

import "strings"

// sanitize replaces characters unsuitable for filenames.
func sanitize(s string) string {
	return strings.NewReplacer("/", "-", " ", "-").Replace(s)
}
