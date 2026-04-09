package activity

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
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

// E-PENPAL-ACTIVITY-PERSIST: verifies Save writes and Load restores activity
// including nested paths where FileName must be recomputed from FilePath.
func TestSaveAndLoad(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "activity.json")

	tr := New()
	ts := time.Date(2025, 6, 15, 10, 0, 0, 0, time.UTC)
	tr.RecordAt(FileViewed, "proj", "thoughts/plans/roadmap.md", ts)
	tr.Record(Comment, "proj", "deep/nested/dir/notes.md")

	if err := tr.Save(path); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// Verify file was created
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("activity.json not created: %v", err)
	}

	// Load into a fresh tracker
	tr2 := New()
	if err := tr2.Load(path); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	files := tr2.RecentFiles(10)
	if len(files) != 2 {
		t.Fatalf("expected 2 files after load, got %d", len(files))
	}

	fa := tr2.Lookup("proj", "thoughts/plans/roadmap.md")
	if fa == nil {
		t.Fatal("expected roadmap.md activity after load")
	}
	if fa.Type != FileViewed {
		t.Errorf("expected type %q, got %q", FileViewed, fa.Type)
	}
	if !fa.Timestamp.Equal(ts) {
		t.Errorf("expected timestamp %v, got %v", ts, fa.Timestamp)
	}
	if fa.FileName != "roadmap.md" {
		t.Errorf("expected FileName %q after round-trip, got %q", "roadmap.md", fa.FileName)
	}

	fb := tr2.Lookup("proj", "deep/nested/dir/notes.md")
	if fb == nil {
		t.Fatal("expected notes.md activity after load")
	}
	if fb.Type != Comment {
		t.Errorf("expected type %q, got %q", Comment, fb.Type)
	}
	if fb.FileName != "notes.md" {
		t.Errorf("expected FileName %q after round-trip, got %q", "notes.md", fb.FileName)
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies Load is a no-op when the file doesn't exist.
func TestLoadMissingFileIsNoOp(t *testing.T) {
	tr := New()
	err := tr.Load(filepath.Join(t.TempDir(), "nonexistent.json"))
	if err != nil {
		t.Fatalf("Load of missing file should return nil, got: %v", err)
	}
	if len(tr.RecentFiles(10)) != 0 {
		t.Error("expected empty tracker after loading missing file")
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies Load uses RecordAt semantics (doesn't overwrite runtime events).
func TestLoadDoesNotOverwriteRuntimeEvents(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "activity.json")

	// Save a "viewed" event
	tr := New()
	old := time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)
	tr.RecordAt(FileViewed, "p1", "a.md", old)
	if err := tr.Save(path); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// New tracker with a runtime event for the same file
	tr2 := New()
	tr2.Record(Comment, "p1", "a.md")

	// Load should not overwrite the runtime event
	if err := tr2.Load(path); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	fa := tr2.Lookup("p1", "a.md")
	if fa.Type != Comment {
		t.Errorf("expected runtime event %q preserved, got %q", Comment, fa.Type)
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies Save is atomic (no partial writes).
func TestSaveIsAtomic(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "activity.json")

	tr := New()
	tr.Record(FileModified, "p1", "a.md")
	if err := tr.Save(path); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// Verify no .tmp file left behind
	if _, err := os.Stat(path + ".tmp"); !os.IsNotExist(err) {
		t.Error("expected .tmp file to be cleaned up after save")
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies Load returns error on corrupt JSON without panicking.
func TestLoadCorruptJSON(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "activity.json")
	os.WriteFile(path, []byte(`{not valid json`), 0644)

	tr := New()
	err := tr.Load(path)
	if err == nil {
		t.Fatal("expected error from corrupt JSON, got nil")
	}
	// Tracker should remain empty — no partial state
	if len(tr.RecentFiles(10)) != 0 {
		t.Error("expected empty tracker after corrupt load")
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies Save prunes to maxPersistedEntries.
func TestSavePrunesOldEntries(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "activity.json")

	tr := New()
	base := time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)
	for i := 0; i < maxPersistedEntries+100; i++ {
		tr.RecordAt(FileModified, "p1", filepath.Join("dir", fmt.Sprintf("file%d.md", i)), base.Add(time.Duration(i)*time.Second))
	}

	if err := tr.Save(path); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	tr2 := New()
	if err := tr2.Load(path); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	files := tr2.RecentFiles(0)
	if len(files) != maxPersistedEntries {
		t.Errorf("expected %d entries after pruned save, got %d", maxPersistedEntries, len(files))
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies Save creates parent directories.
func TestSaveCreatesParentDir(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "sub", "deep", "activity.json")

	tr := New()
	tr.Record(FileViewed, "p1", "a.md")

	if err := tr.Save(path); err != nil {
		t.Fatalf("Save should create parent dirs, got: %v", err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("file not created: %v", err)
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies SetOnChange callback fires on Record().
func TestOnChangeCalledOnRecord(t *testing.T) {
	tr := New()
	var count atomic.Int32
	tr.SetOnChange(func() { count.Add(1) })

	tr.Record(FileViewed, "p1", "a.md")
	tr.Record(FileModified, "p1", "b.md")

	if got := count.Load(); got != 2 {
		t.Errorf("expected onChange called 2 times, got %d", got)
	}
}

// E-PENPAL-ACTIVITY-PERSIST: verifies RecordAt does NOT fire onChange (seed-only).
func TestOnChangeNotCalledOnRecordAt(t *testing.T) {
	tr := New()
	var count atomic.Int32
	tr.SetOnChange(func() { count.Add(1) })

	tr.RecordAt(FileModified, "p1", "a.md", time.Now())

	if got := count.Load(); got != 0 {
		t.Errorf("expected onChange not called for RecordAt, got %d calls", got)
	}
}
