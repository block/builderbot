package comments

import (
	"testing"

	"github.com/loganj/penpal/internal/cache"
)

// newMinimalStore creates a Store without a backing project on disk,
// suitable for testing in-memory-only functionality like heartbeats.
func newMinimalStore() *Store {
	return NewStore(cache.New(), nil)
}

// E-PENPAL-HEARTBEAT: RecordHeartbeat then IsAgentActive returns true.
func TestRecordHeartbeatThenActive(t *testing.T) {
	s := newMinimalStore()
	s.RecordHeartbeat("proj", "file.md")

	if !s.IsAgentActive("proj", "file.md") {
		t.Error("expected IsAgentActive to return true after RecordHeartbeat")
	}
}

// E-PENPAL-HEARTBEAT: IsAgentActive returns false for unrecorded project.
func TestIsAgentActiveUnrecorded(t *testing.T) {
	s := newMinimalStore()

	if s.IsAgentActive("unknown", "file.md") {
		t.Error("expected IsAgentActive to return false for unrecorded project")
	}
}

// E-PENPAL-HEARTBEAT: IsAgentActive returns false for recorded project but different file.
func TestIsAgentActiveDifferentFile(t *testing.T) {
	s := newMinimalStore()
	s.RecordHeartbeat("proj", "file1.md")

	if s.IsAgentActive("proj", "file2.md") {
		t.Error("expected IsAgentActive to return false for different file path")
	}
}

// E-PENPAL-HEARTBEAT: ClearProjectHeartbeats clears all heartbeats for a project.
func TestClearProjectHeartbeats(t *testing.T) {
	s := newMinimalStore()

	// Record heartbeats for multiple files in the same project
	s.RecordHeartbeat("proj", "file1.md")
	s.RecordHeartbeat("proj", "file2.md")
	s.RecordHeartbeat("proj", "sub/file3.md")

	// Also record a heartbeat for a different project
	s.RecordHeartbeat("other", "doc.md")

	// Verify all are active
	if !s.IsAgentActive("proj", "file1.md") {
		t.Fatal("setup: expected proj/file1.md to be active")
	}
	if !s.IsAgentActive("proj", "file2.md") {
		t.Fatal("setup: expected proj/file2.md to be active")
	}
	if !s.IsAgentActive("proj", "sub/file3.md") {
		t.Fatal("setup: expected proj/sub/file3.md to be active")
	}

	// Clear heartbeats for "proj"
	s.ClearProjectHeartbeats("proj")

	// All "proj" heartbeats should be gone
	if s.IsAgentActive("proj", "file1.md") {
		t.Error("expected proj/file1.md to be inactive after ClearProjectHeartbeats")
	}
	if s.IsAgentActive("proj", "file2.md") {
		t.Error("expected proj/file2.md to be inactive after ClearProjectHeartbeats")
	}
	if s.IsAgentActive("proj", "sub/file3.md") {
		t.Error("expected proj/sub/file3.md to be inactive after ClearProjectHeartbeats")
	}

	// The "other" project should still be active
	if !s.IsAgentActive("other", "doc.md") {
		t.Error("expected other/doc.md to remain active after clearing 'proj'")
	}
}

// E-PENPAL-HEARTBEAT: ClearProjectHeartbeats is a no-op when project has no heartbeats.
func TestClearProjectHeartbeatsNoop(t *testing.T) {
	s := newMinimalStore()
	s.RecordHeartbeat("keep", "file.md")

	// Clearing a nonexistent project should not panic or affect others
	s.ClearProjectHeartbeats("nonexistent")

	if !s.IsAgentActive("keep", "file.md") {
		t.Error("expected 'keep' heartbeat to survive clearing nonexistent project")
	}
}

// E-PENPAL-HEARTBEAT: IsProjectActive returns true when any file heartbeat exists.
func TestIsProjectActive(t *testing.T) {
	s := newMinimalStore()

	if s.IsProjectActive("proj") {
		t.Error("expected IsProjectActive to return false before any heartbeats")
	}

	s.RecordHeartbeat("proj", "file.md")
	if !s.IsProjectActive("proj") {
		t.Error("expected IsProjectActive to return true after RecordHeartbeat")
	}
}
