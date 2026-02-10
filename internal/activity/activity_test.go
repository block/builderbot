package activity

import (
	"sync"
	"testing"
	"time"
)

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

func TestLookupReturnsNilForUntracked(t *testing.T) {
	tr := New()
	fa := tr.Lookup("noproject", "nofile.md")
	if fa != nil {
		t.Errorf("expected nil, got %+v", fa)
	}
}

func TestLookupReturnsCopy(t *testing.T) {
	tr := New()
	tr.Record(FileViewed, "p1", "a.md")

	fa1 := tr.Lookup("p1", "a.md")
	fa2 := tr.Lookup("p1", "a.md")
	if fa1 == fa2 {
		t.Error("expected different pointers (copies), got same pointer")
	}
}

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
