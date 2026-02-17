package comments

import "sort"

// OrderComments sorts comments within a thread by reply relationship
// and timestamp. Comments are grouped under their parent (InReplyTo),
// with siblings sorted by CreatedAt. Comments without InReplyTo are
// treated as root-level, sorted by timestamp.
func OrderComments(comments []Comment) []Comment {
	if len(comments) <= 1 {
		return comments
	}

	byID := make(map[string]struct{}, len(comments))
	for _, c := range comments {
		byID[c.ID] = struct{}{}
	}

	// Build parent -> children map. A comment is a root if it has no
	// InReplyTo or its parent doesn't exist in this thread.
	children := make(map[string][]Comment) // parentID -> children
	var roots []Comment
	for _, c := range comments {
		parent := c.InReplyTo
		if parent != "" {
			if _, ok := byID[parent]; ok {
				children[parent] = append(children[parent], c)
				continue
			}
		}
		roots = append(roots, c)
	}

	byTime := func(a, b Comment) bool {
		return a.CreatedAt.Before(b.CreatedAt)
	}

	sort.Slice(roots, func(i, j int) bool { return byTime(roots[i], roots[j]) })
	for k := range children {
		kids := children[k]
		sort.Slice(kids, func(i, j int) bool { return byTime(kids[i], kids[j]) })
		children[k] = kids
	}

	result := make([]Comment, 0, len(comments))
	var walk func(id string)
	walk = func(id string) {
		for _, c := range children[id] {
			result = append(result, c)
			walk(c.ID)
		}
	}
	for _, r := range roots {
		result = append(result, r)
		walk(r.ID)
	}

	return result
}
