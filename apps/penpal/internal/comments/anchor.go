package comments

import "strings"

// ResolveAnchor finds the byte offset of the anchor's selected text within
// the markdown source. Returns -1 if not found.
//
// Strategy:
// 1. Exact match of SelectedText -> use first occurrence
// 2. Multiple matches -> use Before/After context to disambiguate
// 3. No match -> return -1 (thread is orphaned)
//
// E-PENPAL-ANCHOR-RESOLVE: text matching with Before/After disambiguation.
func ResolveAnchor(markdown string, anchor Anchor) int {
	if anchor.SelectedText == "" {
		return -1
	}

	count := strings.Count(markdown, anchor.SelectedText)
	if count == 0 {
		return -1
	}

	if count == 1 {
		return strings.Index(markdown, anchor.SelectedText)
	}

	// Multiple matches: disambiguate using Before/After context
	if anchor.Before != "" || anchor.After != "" {
		offset := 0
		for i := 0; i < count; i++ {
			idx := strings.Index(markdown[offset:], anchor.SelectedText)
			if idx == -1 {
				break
			}
			absIdx := offset + idx

			beforeMatch := true
			afterMatch := true

			if anchor.Before != "" {
				if absIdx >= len(anchor.Before) {
					preceding := markdown[absIdx-len(anchor.Before) : absIdx]
					beforeMatch = preceding == anchor.Before
				} else {
					beforeMatch = false
				}
			}

			if anchor.After != "" {
				afterStart := absIdx + len(anchor.SelectedText)
				if afterStart+len(anchor.After) <= len(markdown) {
					following := markdown[afterStart : afterStart+len(anchor.After)]
					afterMatch = following == anchor.After
				} else {
					afterMatch = false
				}
			}

			if beforeMatch && afterMatch {
				return absIdx
			}

			offset = absIdx + 1
		}
	}

	// Fall back to first occurrence if context didn't help
	return strings.Index(markdown, anchor.SelectedText)
}

// ResolveAnchorsToLines takes threads and markdown source, returns a map
// of threadID -> line number (1-indexed). Threads that can't be anchored
// are mapped to -1.
//
// StartLine is the primary mechanism — it records which rendered element the
// comment was attached to. Text matching against raw markdown is only used
// as backward compat for old anchors that lack StartLine.
//
// E-PENPAL-ANCHOR-RESOLVE: maps threads to line numbers via StartLine or text fallback.
func ResolveAnchorsToLines(threads []Thread, markdown string) map[string]int {
	result := make(map[string]int, len(threads))
	for _, t := range threads {
		if t.Anchor.StartLine > 0 {
			result[t.ID] = t.Anchor.StartLine
			continue
		}
		// Backward compat: text matching for anchors without StartLine
		offset := ResolveAnchor(markdown, t.Anchor)
		if offset < 0 {
			result[t.ID] = -1
		} else {
			line := 1
			for i := 0; i < offset && i < len(markdown); i++ {
				if markdown[i] == '\n' {
					line++
				}
			}
			result[t.ID] = line
		}
	}
	return result
}
