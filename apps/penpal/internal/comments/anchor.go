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
// Always re-resolves anchors against the current markdown using text matching
// (selectedText + before/after context). The stored StartLine is used as a
// fallback only when text matching fails, preserving highlights even when the
// selected text has been edited.
//
// E-PENPAL-ANCHOR-RESOLVE: maps threads to line numbers via text matching with StartLine fallback.
func ResolveAnchorsToLines(threads []Thread, markdown string) map[string]int {
	result := make(map[string]int, len(threads))
	for _, t := range threads {
		// Always try text matching first so line numbers track document edits
		offset := ResolveAnchor(markdown, t.Anchor)
		if offset >= 0 {
			line := 1
			for i := 0; i < offset && i < len(markdown); i++ {
				if markdown[i] == '\n' {
					line++
				}
			}
			result[t.ID] = line
			continue
		}
		// Text not found — fall back to stored StartLine (text may have been edited)
		if t.Anchor.StartLine > 0 {
			result[t.ID] = t.Anchor.StartLine
			continue
		}
		result[t.ID] = -1
	}
	return result
}
