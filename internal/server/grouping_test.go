package server

import (
	"testing"
	"time"

	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/discovery"
)

func TestBuildFileGroups_RP1Grouped(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "rp1", Type: "tree", RootPath: "/tmp/test/.rp1", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "rp1", Path: "context/index.md", FullPath: ".rp1/context/index.md", Name: "index.md", FileType: "knowledge", ModTime: time.Now()},
		{Source: "rp1", Path: "work/prds/my-prd.md", FullPath: ".rp1/work/prds/my-prd.md", Name: "my-prd.md", FileType: "prd", ModTime: time.Now()},
		{Source: "rp1", Path: "work/features/auth/requirements.md", FullPath: ".rp1/work/features/auth/requirements.md", Name: "requirements.md", FileType: "requirement", ModTime: time.Now()},
		{Source: "rp1", Path: "work/features/auth/design.md", FullPath: ".rp1/work/features/auth/design.md", Name: "design.md", FileType: "design", ModTime: time.Now()},
		{Source: "rp1", Path: "work/features/data-layer/tasks.md", FullPath: ".rp1/work/features/data-layer/tasks.md", Name: "tasks.md", FileType: "task", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// Should have 4 flat groups: Context, PRDs, auth, data-layer
	if len(groups) != 4 {
		t.Fatalf("expected 4 groups, got %d", len(groups))
	}

	expectedGroups := []struct {
		name      string
		source    string
		auto      bool
		fileCount int
	}{
		{"Blueprint", "rp1", true, 1},
		{"auth", "rp1", true, 2},
		{"data-layer", "rp1", true, 1},
		{"Context", "rp1", true, 1},
	}

	for i, eg := range expectedGroups {
		if groups[i].Name != eg.name {
			t.Errorf("group %d: expected name %q, got %q", i, eg.name, groups[i].Name)
		}
		if groups[i].Source != eg.source {
			t.Errorf("group %d: expected source %q, got %q", i, eg.source, groups[i].Source)
		}
		if groups[i].Auto != eg.auto {
			t.Errorf("group %d: expected auto=%v, got %v", i, eg.auto, groups[i].Auto)
		}
		if len(groups[i].Files) != eg.fileCount {
			t.Errorf("group %d (%s): expected %d files, got %d", i, eg.name, eg.fileCount, len(groups[i].Files))
		}
	}
}

func TestBuildFileGroups_ThoughtsFlat(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", RootPath: "/tmp/test/thoughts", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plans/foo.md", FullPath: "thoughts/plans/foo.md", Name: "foo.md", FileType: "plan", ModTime: time.Now()},
		{Source: "thoughts", Path: "research/bar.md", FullPath: "thoughts/research/bar.md", Name: "bar.md", FileType: "research", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// Thoughts has no GroupFiles, so single flat group named "thoughts"
	if len(groups) != 1 {
		t.Fatalf("expected 1 group, got %d", len(groups))
	}
	if groups[0].Name != "thoughts" {
		t.Errorf("expected group name 'thoughts', got %q", groups[0].Name)
	}
	if groups[0].Source != "thoughts" {
		t.Errorf("expected source 'thoughts', got %q", groups[0].Source)
	}
	if len(groups[0].Files) != 2 {
		t.Errorf("expected 2 files, got %d", len(groups[0].Files))
	}
}

func TestBuildFileGroups_MultipleSources(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", RootPath: "/tmp/test/thoughts", Auto: true},
			{Name: "rp1", Type: "tree", RootPath: "/tmp/test/.rp1", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plan.md", FullPath: "thoughts/plan.md", Name: "plan.md", FileType: "plan", ModTime: time.Now()},
		{Source: "rp1", Path: "context/index.md", FullPath: ".rp1/context/index.md", Name: "index.md", FileType: "knowledge", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// thoughts → 1 flat group; rp1 → 1 group (Context only)
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
	}

	if groups[0].Name != "thoughts" {
		t.Errorf("expected first group 'thoughts', got %q", groups[0].Name)
	}
	if groups[1].Name != "Context" {
		t.Errorf("expected second group 'Context', got %q", groups[1].Name)
	}
}

func TestBuildFileGroups_EmptySourceSkipped(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", RootPath: "/tmp/test/thoughts", Auto: true},
			{Name: "rp1", Type: "tree", RootPath: "/tmp/test/.rp1", Auto: true},
		},
	}

	// Only thoughts has files, rp1 is empty
	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plan.md", FullPath: "thoughts/plan.md", Name: "plan.md", FileType: "plan", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	if len(groups) != 1 {
		t.Fatalf("expected 1 group (empty rp1 skipped), got %d", len(groups))
	}
	if groups[0].Name != "thoughts" {
		t.Errorf("expected group 'thoughts', got %q", groups[0].Name)
	}
}
