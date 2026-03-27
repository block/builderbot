package cache

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/loganj/penpal/internal/discovery"
)

// E-PENPAL-PATH-MATCH: verifies FindProjectByPathWithWorktree resolves worktree paths.
func TestFindProjectByPathWithWorktree(t *testing.T) {
	c := New()

	// Set up a project with worktrees
	c.SetProjects([]discovery.Project{
		{
			Name:          "myrepo",
			Path:          "/home/user/Development/myrepo",
			Origin:        "workspace",
			WorkspaceName: "Development",
			Worktrees: []discovery.Worktree{
				{Name: "myrepo", Path: "/home/user/Development/myrepo", Branch: "main", IsMain: true},
				{Name: "fancy-name", Path: "/home/user/Development/myrepo/.claude/worktrees/fancy-name", Branch: "feature-branch"},
				{Name: "external-wt", Path: "/tmp/external-worktree", Branch: "other-branch"},
			},
		},
	})

	tests := []struct {
		name         string
		absPath      string
		wantProject  string
		wantWorktree string
	}{
		{
			name:         "main project root",
			absPath:      "/home/user/Development/myrepo",
			wantProject:  "Development/myrepo",
			wantWorktree: "",
		},
		{
			name:         "file in main project",
			absPath:      "/home/user/Development/myrepo/thoughts/plan.md",
			wantProject:  "Development/myrepo",
			wantWorktree: "",
		},
		{
			name:         "file in nested worktree",
			absPath:      "/home/user/Development/myrepo/.claude/worktrees/fancy-name/thoughts/plan.md",
			wantProject:  "Development/myrepo",
			wantWorktree: "fancy-name",
		},
		{
			name:         "worktree root",
			absPath:      "/home/user/Development/myrepo/.claude/worktrees/fancy-name",
			wantProject:  "Development/myrepo",
			wantWorktree: "fancy-name",
		},
		{
			name:         "external worktree",
			absPath:      "/tmp/external-worktree/src/main.go",
			wantProject:  "Development/myrepo",
			wantWorktree: "external-wt",
		},
		{
			name:         "unrelated path",
			absPath:      "/home/user/other/project",
			wantProject:  "",
			wantWorktree: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			project, worktree := c.FindProjectByPathWithWorktree(tt.absPath)
			if tt.wantProject == "" {
				if project != nil {
					t.Errorf("expected nil project, got %s", project.QualifiedName())
				}
				return
			}
			if project == nil {
				t.Fatalf("expected project %s, got nil", tt.wantProject)
			}
			if project.QualifiedName() != tt.wantProject {
				t.Errorf("project = %s, want %s", project.QualifiedName(), tt.wantProject)
			}
			if worktree != tt.wantWorktree {
				t.Errorf("worktree = %q, want %q", worktree, tt.wantWorktree)
			}
		})
	}
}

// E-PENPAL-SCAN: verifies worktree source remapping and file presence checks.
func TestScanProjectSourcesForWorktree(t *testing.T) {
	// Set up a main project with a "thoughts" tree source and a "manual" files source
	mainDir := t.TempDir()
	wtDir := t.TempDir()

	// Create thoughts dirs and files in both main and worktree
	os.MkdirAll(filepath.Join(mainDir, "thoughts"), 0o755)
	os.MkdirAll(filepath.Join(wtDir, "thoughts"), 0o755)
	os.WriteFile(filepath.Join(mainDir, "thoughts", "main-only.md"), []byte("# Main Only\n"), 0o644)
	os.WriteFile(filepath.Join(wtDir, "thoughts", "wt-only.md"), []byte("# WT Only\n"), 0o644)
	os.WriteFile(filepath.Join(mainDir, "thoughts", "shared.md"), []byte("# Shared Main\n"), 0o644)
	os.WriteFile(filepath.Join(wtDir, "thoughts", "shared.md"), []byte("# Shared WT\n"), 0o644)

	// Create a manually-added file that exists only in main
	os.WriteFile(filepath.Join(mainDir, "manual.md"), []byte("# Manual\n"), 0o644)
	// And one that exists in both
	os.WriteFile(filepath.Join(mainDir, "both.md"), []byte("# Both Main\n"), 0o644)
	os.WriteFile(filepath.Join(wtDir, "both.md"), []byte("# Both WT\n"), 0o644)

	project := &discovery.Project{
		Name:          "test",
		Path:          mainDir,
		Origin:        "workspace",
		WorkspaceName: "ws",
		Sources: []discovery.FileSource{
			{
				Name:           "thoughts",
				Type:           "tree",
				SourceTypeName: "manual",
				RootPath:       filepath.Join(mainDir, "thoughts"),
			},
			{
				Name:           "manual files",
				Type:           "files",
				SourceTypeName: "manual",
				Files: []string{
					filepath.Join(mainDir, "manual.md"),
					filepath.Join(mainDir, "both.md"),
				},
			},
		},
	}

	files := ScanProjectSourcesForWorktree(project, wtDir)

	// Collect file names
	names := map[string]bool{}
	for _, f := range files {
		names[f.Name] = true
	}

	// Should include worktree's thoughts files
	if !names["wt-only.md"] {
		t.Error("expected wt-only.md from worktree thoughts dir")
	}
	if !names["shared.md"] {
		t.Error("expected shared.md from worktree thoughts dir")
	}
	// Should NOT include main-only thoughts file
	if names["main-only.md"] {
		t.Error("main-only.md should not appear in worktree scan")
	}
	// Should include manually-added file that exists in worktree
	if !names["both.md"] {
		t.Error("expected both.md (exists in worktree)")
	}
	// Should NOT include manually-added file that only exists in main
	if names["manual.md"] {
		t.Error("manual.md should not appear in worktree scan (doesn't exist in worktree)")
	}
}

// E-PENPAL-CACHE: verifies WorktreePath returns correct filesystem paths for worktrees.
func TestWorktreePath(t *testing.T) {
	c := New()

	c.SetProjects([]discovery.Project{
		{
			Name:          "myrepo",
			Path:          "/home/user/myrepo",
			Origin:        "workspace",
			WorkspaceName: "Dev",
			Worktrees: []discovery.Worktree{
				{Name: "myrepo", Path: "/home/user/myrepo", Branch: "main", IsMain: true},
				{Name: "fancy", Path: "/home/user/myrepo/.claude/worktrees/fancy", Branch: "feat"},
			},
		},
	})

	tests := []struct {
		name     string
		project  string
		worktree string
		want     string
	}{
		{"empty worktree returns project path", "Dev/myrepo", "", "/home/user/myrepo"},
		{"known worktree", "Dev/myrepo", "fancy", "/home/user/myrepo/.claude/worktrees/fancy"},
		{"unknown worktree", "Dev/myrepo", "nonexistent", ""},
		{"unknown project", "Dev/other", "", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := c.WorktreePath(tt.project, tt.worktree)
			if got != tt.want {
				t.Errorf("WorktreePath(%q, %q) = %q, want %q", tt.project, tt.worktree, got, tt.want)
			}
		})
	}
}
