package comments

import (
	"testing"
	"time"
)

func makeComment(id, inReplyTo string, seconds int) Comment {
	return Comment{
		ID:        id,
		Author:    "user",
		Role:      "human",
		Body:      "comment " + id,
		CreatedAt: time.Date(2025, 1, 1, 0, 0, seconds, 0, time.UTC),
		InReplyTo: inReplyTo,
	}
}

func ids(cs []Comment) []string {
	out := make([]string, len(cs))
	for i, c := range cs {
		out[i] = c.ID
	}
	return out
}

func assertOrder(t *testing.T, got []Comment, want []string) {
	t.Helper()
	gotIDs := ids(got)
	if len(gotIDs) != len(want) {
		t.Fatalf("length mismatch: got %v, want %v", gotIDs, want)
	}
	for i := range want {
		if gotIDs[i] != want[i] {
			t.Errorf("index %d: got %q, want %q (full: %v)", i, gotIDs[i], want[i], gotIDs)
			return
		}
	}
}

func TestOrderComments_Empty(t *testing.T) {
	result := OrderComments(nil)
	if len(result) != 0 {
		t.Errorf("expected empty, got %d", len(result))
	}
}

func TestOrderComments_Single(t *testing.T) {
	cs := []Comment{makeComment("a", "", 1)}
	result := OrderComments(cs)
	assertOrder(t, result, []string{"a"})
}

func TestOrderComments_RootsOnly(t *testing.T) {
	cs := []Comment{
		makeComment("c", "", 3),
		makeComment("a", "", 1),
		makeComment("b", "", 2),
	}
	result := OrderComments(cs)
	assertOrder(t, result, []string{"a", "b", "c"})
}

func TestOrderComments_LinearReplies(t *testing.T) {
	cs := []Comment{
		makeComment("a", "", 1),
		makeComment("b", "a", 2),
		makeComment("c", "b", 3),
	}
	result := OrderComments(cs)
	assertOrder(t, result, []string{"a", "b", "c"})
}

func TestOrderComments_InterleavedReplies(t *testing.T) {
	// Two roots with interleaved children
	cs := []Comment{
		makeComment("a", "", 1),
		makeComment("b", "", 2),
		makeComment("c", "a", 3), // reply to a
		makeComment("d", "b", 4), // reply to b
	}
	result := OrderComments(cs)
	// Should group: a, c (child of a), b, d (child of b)
	assertOrder(t, result, []string{"a", "c", "b", "d"})
}

func TestOrderComments_MissingParentFallback(t *testing.T) {
	// "b" references a parent that doesn't exist in the thread
	cs := []Comment{
		makeComment("a", "", 1),
		makeComment("b", "nonexistent", 2),
		makeComment("c", "a", 3),
	}
	result := OrderComments(cs)
	// "b" becomes a root (missing parent fallback), sorted by time
	assertOrder(t, result, []string{"a", "c", "b"})
}

func TestOrderComments_DeepNesting(t *testing.T) {
	cs := []Comment{
		makeComment("a", "", 1),
		makeComment("b", "a", 2),
		makeComment("c", "b", 3),
		makeComment("d", "c", 4),
	}
	result := OrderComments(cs)
	assertOrder(t, result, []string{"a", "b", "c", "d"})
}

func TestOrderComments_SiblingSortByTime(t *testing.T) {
	// Multiple children of the same parent, out of order
	cs := []Comment{
		makeComment("a", "", 1),
		makeComment("d", "a", 4),
		makeComment("b", "a", 2),
		makeComment("c", "a", 3),
	}
	result := OrderComments(cs)
	assertOrder(t, result, []string{"a", "b", "c", "d"})
}
