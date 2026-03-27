package activity

import (
	"sync"
	"testing"
	"time"
)

// E-PENPAL-ACTIVITY: verifies Record stores activity with correct fields.
func TestRecordStoresActivity(t *testing.T) {
	tr := New()
	tr.Record(FileViewed, "myproject", "thoughts/plan.md")

	fa := tr.Lookup("myproject", "thoughts/plan.md")
	if fa == nil {
		t.Fatal("expected activity, got nil")
	}
	if fa.Type != FileViewed {
		t.Errorf("expected type %q, got %q", FileViewed, fa.Type)
	}
	if fa.Project != "myproject" {
		t.Errorf("expected project %q, got %q", "myproject", fa.Project)
	}
	if fa.FilePath != "thoughts/plan.md" {
		t.Errorf("expected filePath %q, got %q", "thoughts/plan.md", fa.FilePath)
	}
	if fa.FileName != "plan.md" {
		t.Errorf("expected fileName %q, got %q", "plan.md", fa.FileName)
	}
}

// E-PENPAL-ACTIVITY: verifies Record overwrites previous events for same file.
func TestRecordOverwritesPrevious(t *testing.T) {
	tr := New()
	tr.Record(FileViewed, "myproject", "thoughts/plan.md")
	time.Sleep(time.Millisecond)
	tr.Record(FileModified, "myproject", "thoughts/plan.md")

	fa := tr.Lookup("myproject", "thoughts/plan.md")
	if fa == nil {
		t.Fatal("expected activity, got nil")
	}
	if fa.Type != FileModified {
		t.Errorf("expected type %q, got %q", FileModified, fa.Type)
	}
}

// E-PENPAL-ACTIVITY: verifies RecentFiles returns most-recent-first order.
func TestRecentFilesOrder(t *testing.T) {
	tr := New()
	tr.Record(FileViewed, "p1", "a.md")
	time.Sleep(time.Millisecond)
	tr.Record(FileModified, "p1", "b.md")
	time.Sleep(time.Millisecond)
	tr.Record(Comment, "p2", "c.md")

	files := tr.RecentFiles(10)
	if len(files) != 3 {
		t.Fatalf("expected 3 files, got %d", len(files))
	}
	if files[0].FilePath != "c.md" {
		t.Errorf("expected c.md first, got %s", files[0].FilePath)
	}
	if files[1].FilePath != "b.md" {
		t.Errorf("expected b.md second, got %s", files[1].FilePath)
	}
	if files[2].FilePath != "a.md" {
		t.Errorf("expected a.md third, got %s", files[2].FilePath)
	}
}

// E-PENPAL-ACTIVITY: verifies RecentFiles respects the limit parameter.
func TestRecentFilesLimit(t *testing.T) {
	tr := New()
	tr.Record(FileViewed, "p1", "a.md")
	time.Sleep(time.Millisecond)
	tr.Record(FileModified, "p1", "b.md")
	time.Sleep(time.Millisecond)
	tr.Record(Comment, "p2", "c.md")

	files := tr.RecentFiles(2)
	if len(files) != 2 {
		t.Fatalf("expected 2 files, got %d", len(files))
	}
}

// E-PENPAL-ACTIVITY: verifies Lookup returns nil for untracked files.
func TestLookupReturnsNilForUntracked(t *testing.T) {
	tr := New()
	fa := tr.Lookup("noproject", "nofile.md")
	if fa != nil {
		t.Errorf("expected nil, got %+v", fa)
	}
}

// E-PENPAL-ACTIVITY: verifies Lookup returns a copy, not the original pointer.
func TestLookupReturnsCopy(t *testing.T) {
	tr := New()
	tr.Record(FileViewed, "p1", "a.md")

	fa1 := tr.Lookup("p1", "a.md")
	fa2 := tr.Lookup("p1", "a.md")
	if fa1 == fa2 {
		t.Error("expected different pointers (copies), got same pointer")
	}
}

// E-PENPAL-ACTIVITY: verifies RecordAt stores activity with explicit timestamp.
func TestRecordAtSetsTimestamp(t *testing.T) {
	tr := New()
	ts := time.Date(2025, 1, 15, 10, 30, 0, 0, time.UTC)
	tr.RecordAt(FileModified, "p1", "thoughts/plan.md", ts)

	fa := tr.Lookup("p1", "thoughts/plan.md")
	if fa == nil {
		t.Fatal("expected activity, got nil")
	}
	if fa.Type != FileModified {
		t.Errorf("expected type %q, got %q", FileModified, fa.Type)
	}
	if !fa.Timestamp.Equal(ts) {
		t.Errorf("expected timestamp %v, got %v", ts, fa.Timestamp)
	}
	if fa.FileName != "plan.md" {
		t.Errorf("expected fileName %q, got %q", "plan.md", fa.FileName)
	}
}

// E-PENPAL-ACTIVITY: verifies RecordAt does not overwrite existing events.
func TestRecordAtDoesNotOverwrite(t *testing.T) {
	tr := New()
	// Record a real event first
	tr.Record(FileCreated, "p1", "a.md")

	// Try to seed with RecordAt — should be ignored
	old := time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)
	tr.RecordAt(FileModified, "p1", "a.md", old)

	fa := tr.Lookup("p1", "a.md")
	if fa.Type != FileCreated {
		t.Errorf("expected original type %q preserved, got %q", FileCreated, fa.Type)
	}
	if fa.Timestamp.Equal(old) {
		t.Error("RecordAt should not have overwritten the existing timestamp")
	}
}

// E-PENPAL-ACTIVITY: verifies concurrent Record/Lookup/RecentFiles are safe.
func TestConcurrentAccess(t *testing.T) {
	tr := New()
	var wg sync.WaitGroup

	// Writers
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			tr.Record(FileModified, "p1", "file.md")
		}(i)
	}

	// Readers
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			tr.RecentFiles(10)
			tr.Lookup("p1", "file.md")
		}()
	}

	wg.Wait()
}
