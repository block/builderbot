package server

import (
	"testing"
	"time"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-API-ROUTES: verifies RP1 source produces grouped file sections.
func TestBuildFileGroups_RP1Grouped(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "rp1", Type: "tree", SourceTypeName: "rp1", RootPath: "/tmp/test/.rp1", Auto: true},
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

	// Should have 4 typed groups + 1 "All Markdown" virtual group
	if len(groups) != 5 {
		t.Fatalf("expected 5 groups, got %d", len(groups))
	}

	expectedGroups := []struct {
		name      string
		source    string
		auto      bool
		fileCount int
	}{
		{"Blueprint", "rp1", true, 1},
		{"Feature: auth", "rp1", true, 2},
		{"Feature: data-layer", "rp1", true, 1},
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

// E-PENPAL-API-ROUTES, E-PENPAL-SRC-THOUGHTS: verifies thoughts source produces a single flat group.
func TestBuildFileGroups_ThoughtsFlat(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test/thoughts", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plans/foo.md", FullPath: "thoughts/plans/foo.md", Name: "foo.md", FileType: "plan", ModTime: time.Now()},
		{Source: "thoughts", Path: "research/bar.md", FullPath: "thoughts/research/bar.md", Name: "bar.md", FileType: "research", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// Thoughts has no GroupFiles, so single flat group + All Markdown
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
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

// E-PENPAL-API-ROUTES: verifies multiple sources produce separate groups.
func TestBuildFileGroups_MultipleSources(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test/thoughts", Auto: true},
			{Name: "rp1", Type: "tree", SourceTypeName: "rp1", RootPath: "/tmp/test/.rp1", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plan.md", FullPath: "thoughts/plan.md", Name: "plan.md", FileType: "plan", ModTime: time.Now()},
		{Source: "rp1", Path: "context/index.md", FullPath: ".rp1/context/index.md", Name: "index.md", FileType: "knowledge", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// thoughts → 1 flat group; rp1 → 1 group (Context only); + All Markdown
	if len(groups) != 3 {
		t.Fatalf("expected 3 groups, got %d", len(groups))
	}

	if groups[0].Name != "thoughts" {
		t.Errorf("expected first group 'thoughts', got %q", groups[0].Name)
	}
	if groups[1].Name != "Context" {
		t.Errorf("expected second group 'Context', got %q", groups[1].Name)
	}
}

// E-PENPAL-API-ROUTES: verifies empty sources are omitted from groups.
func TestBuildFileGroups_EmptySourceSkipped(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test/thoughts", Auto: true},
			{Name: "rp1", Type: "tree", SourceTypeName: "rp1", RootPath: "/tmp/test/.rp1", Auto: true},
		},
	}

	// Only thoughts has files, rp1 is empty
	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plan.md", FullPath: "thoughts/plan.md", Name: "plan.md", FileType: "plan", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// 1 typed group (empty rp1 skipped) + All Markdown
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups (empty rp1 skipped), got %d", len(groups))
	}
	if groups[0].Name != "thoughts" {
		t.Errorf("expected group 'thoughts', got %q", groups[0].Name)
	}
}

// E-PENPAL-ADD-SOURCE, E-PENPAL-SRC-MANUAL: verifies manual source produces directory headings.
func TestBuildFileGroups_ManualSourceDirHeadings(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "docs", Type: "tree", SourceTypeName: "manual", RootPath: "/tmp/test/docs", Auto: false},
		},
	}

	files := []cache.FileInfo{
		{Source: "docs", Path: "root.md", FullPath: "docs/root.md", Name: "root.md", FileType: "other", ModTime: time.Now()},
		{Source: "docs", Path: "guides/setup.md", FullPath: "docs/guides/setup.md", Name: "setup.md", FileType: "other", ModTime: time.Now()},
		{Source: "docs", Path: "guides/deploy.md", FullPath: "docs/guides/deploy.md", Name: "deploy.md", FileType: "other", ModTime: time.Now()},
		{Source: "docs", Path: "api/endpoints.md", FullPath: "docs/api/endpoints.md", Name: "endpoints.md", FileType: "other", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// 1 typed group + All Markdown
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
	}

	g := groups[0]
	if len(g.Files) != 4 {
		t.Fatalf("expected 4 files, got %d", len(g.Files))
	}

	// Root file first (Dir=""), then api/, then guides/ (sorted)
	if g.Files[0].Dir != "" {
		t.Errorf("file 0: expected Dir='', got %q", g.Files[0].Dir)
	}
	if g.Files[1].Dir != "api" || !g.Files[1].ShowDir {
		t.Errorf("file 1: expected Dir='api' ShowDir=true, got Dir=%q ShowDir=%v", g.Files[1].Dir, g.Files[1].ShowDir)
	}
	if g.Files[2].Dir != "guides" || !g.Files[2].ShowDir {
		t.Errorf("file 2: expected Dir='guides' ShowDir=true, got Dir=%q ShowDir=%v", g.Files[2].Dir, g.Files[2].ShowDir)
	}
	if g.Files[3].Dir != "guides" || g.Files[3].ShowDir {
		t.Errorf("file 3: expected Dir='guides' ShowDir=false, got Dir=%q ShowDir=%v", g.Files[3].Dir, g.Files[3].ShowDir)
	}
}

// E-PENPAL-API-ROUTES: verifies file titles flow through to group view.
func TestBuildFileGroups_TitleFlowsThrough(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test/thoughts", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plans/my-plan.md", FullPath: "thoughts/plans/my-plan.md", Name: "my-plan.md", Title: "Per-Tab Navigation", FileType: "plan", ModTime: time.Now()},
		{Source: "thoughts", Path: "research/bar.md", FullPath: "thoughts/research/bar.md", Name: "bar.md", Title: "", FileType: "research", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// 1 typed group + All Markdown
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
	}

	if groups[0].Files[0].Title != "Per-Tab Navigation" {
		t.Errorf("plan file Title = %q, want %q", groups[0].Files[0].Title, "Per-Tab Navigation")
	}
	if groups[0].Files[1].Title != "" {
		t.Errorf("research file Title = %q, want empty", groups[0].Files[1].Title)
	}
}

// E-PENPAL-API-ROUTES, E-PENPAL-SRC-THOUGHTS: verifies thoughts source does not produce directory headings.
func TestBuildFileGroups_ThoughtsNoDirHeadings(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test/thoughts", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plans/foo.md", FullPath: "thoughts/plans/foo.md", Name: "foo.md", FileType: "plan", ModTime: time.Now()},
		{Source: "thoughts", Path: "research/bar.md", FullPath: "thoughts/research/bar.md", Name: "bar.md", FileType: "research", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	// 1 typed group + All Markdown
	if len(groups) != 2 {
		t.Fatalf("expected 2 groups, got %d", len(groups))
	}
	for i, f := range groups[0].Files {
		if f.Dir != "" || f.ShowDir {
			t.Errorf("file %d: thoughts should have no Dir/ShowDir, got Dir=%q ShowDir=%v", i, f.Dir, f.ShowDir)
		}
	}
}

// E-PENPAL-SRC-ALL-MD: verifies "All Markdown" virtual group is always appended.
func TestBuildFileGroups_AllMarkdownVirtual(t *testing.T) {
	project := &discovery.Project{
		Name: "test-project",
		Path: "/tmp/test",
		Sources: []discovery.FileSource{
			{Name: "thoughts", Type: "tree", SourceTypeName: "thoughts", RootPath: "/tmp/test/thoughts", Auto: true},
		},
	}

	files := []cache.FileInfo{
		{Source: "thoughts", Path: "plans/foo.md", FullPath: "thoughts/plans/foo.md", Name: "foo.md", FileType: "plan", ModTime: time.Now()},
		{Source: "thoughts", Path: "research/bar.md", FullPath: "thoughts/research/bar.md", Name: "bar.md", FileType: "research", ModTime: time.Now()},
	}

	groups := buildFileGroups(project, files)

	last := groups[len(groups)-1]
	if last.Name != "All Markdown" {
		t.Fatalf("last group should be 'All Markdown', got %q", last.Name)
	}
	if last.Source != "__all_markdown__" {
		t.Errorf("expected source '__all_markdown__', got %q", last.Source)
	}
	if last.Auto {
		t.Errorf("All Markdown should be auto=false (virtual source, not auto-detected)")
	}
	if len(last.Files) != 2 {
		t.Errorf("expected 2 files in All Markdown, got %d", len(last.Files))
	}
	// Files should be sorted by path
	if last.Files[0].Path > last.Files[1].Path {
		t.Errorf("All Markdown files should be sorted by path: %q > %q", last.Files[0].Path, last.Files[1].Path)
	}
}
