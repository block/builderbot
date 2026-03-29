package discovery

import (
	"os"
	"path/filepath"
	"testing"
)

// E-PENPAL-SRC-RP1: verifies GroupFiles organizes paths into ordered display groups.
func TestGroupRP1Paths(t *testing.T) {
	st := GetSourceType("rp1")
	if st == nil || st.GroupFiles == nil {
		t.Fatal("rp1 source type not registered or has no GroupFiles")
	}

	tests := []struct {
		name     string
		paths    []string
		expected []FileGroup
	}{
		{
			name:     "empty input",
			paths:    nil,
			expected: nil,
		},
		{
			name: "context files",
			paths: []string{
				"context/index.md",
				"context/architecture.md",
			},
			expected: []FileGroup{
				{Name: "Context", Paths: []string{"context/index.md", "context/architecture.md"}},
			},
		},
		{
			name: "feature files grouped by feature ID",
			paths: []string{
				"work/features/auth/requirements.md",
				"work/features/auth/design.md",
				"work/features/data-layer/tasks.md",
			},
			expected: []FileGroup{
				{Name: "Feature: auth", Paths: []string{"work/features/auth/requirements.md", "work/features/auth/design.md"}},
				{Name: "Feature: data-layer", Paths: []string{"work/features/data-layer/tasks.md"}},
			},
		},
		{
			name: "charter and PRDs grouped into Blueprint",
			paths: []string{
				"work/charter.md",
				"work/quick-builds/build-1.md",
				"work/prds/my-prd.md",
				"context/index.md",
			},
			expected: []FileGroup{
				{Name: "Blueprint", Paths: []string{"work/charter.md", "work/prds/my-prd.md"}},
				{Name: "Quick Builds", Paths: []string{"work/quick-builds/build-1.md"}},
				{Name: "Context", Paths: []string{"context/index.md"}},
			},
		},
		{
			name: "features before context, features sorted alphabetically",
			paths: []string{
				"context/index.md",
				"work/features/zebra/tasks.md",
				"work/features/alpha/requirements.md",
				"work/prds/my-prd.md",
			},
			expected: []FileGroup{
				{Name: "Blueprint", Paths: []string{"work/prds/my-prd.md"}},
				{Name: "Feature: alpha", Paths: []string{"work/features/alpha/requirements.md"}},
				{Name: "Feature: zebra", Paths: []string{"work/features/zebra/tasks.md"}},
				{Name: "Context", Paths: []string{"context/index.md"}},
			},
		},
		{
			name: "malformed feature path goes to Other",
			paths: []string{
				"work/features/lonely-file.md",
			},
			expected: []FileGroup{
				{Name: "Other", Paths: []string{"work/features/lonely-file.md"}},
			},
		},
		{
			name: "research, reviews, content, issues get own groups",
			paths: []string{
				"work/research/2025-01-topic.md",
				"work/pr-reviews/123/review.md",
				"work/content/blog/post.md",
				"work/issues/bug-42/investigation_report.md",
			},
			expected: []FileGroup{
				{Name: "Research", Paths: []string{"work/research/2025-01-topic.md"}},
				{Name: "Reviews", Paths: []string{"work/pr-reviews/123/review.md"}},
				{Name: "Content", Paths: []string{"work/content/blog/post.md"}},
				{Name: "Issue: bug-42", Paths: []string{"work/issues/bug-42/investigation_report.md"}},
			},
		},
		{
			name: "full category ordering with all groups",
			paths: []string{
				"context/index.md",
				"work/prds/my-prd.md",
				"work/quick-builds/build-1.md",
				"work/research/topic.md",
				"work/pr-reviews/1/review.md",
				"work/content/blog/post.md",
				"work/issues/bug/report.md",
				"work/charter.md",
				"work/features/auth/tasks.md",
			},
			expected: []FileGroup{
				{Name: "Blueprint", Paths: []string{"work/prds/my-prd.md", "work/charter.md"}},
				{Name: "Quick Builds", Paths: []string{"work/quick-builds/build-1.md"}},
				{Name: "Research", Paths: []string{"work/research/topic.md"}},
				{Name: "Reviews", Paths: []string{"work/pr-reviews/1/review.md"}},
				{Name: "Content", Paths: []string{"work/content/blog/post.md"}},
				{Name: "Issue: bug", Paths: []string{"work/issues/bug/report.md"}},
				{Name: "Feature: auth", Paths: []string{"work/features/auth/tasks.md"}},
				{Name: "Context", Paths: []string{"context/index.md"}},
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := st.GroupFiles(tt.paths)

			if len(got) != len(tt.expected) {
				t.Fatalf("expected %d groups, got %d: %+v", len(tt.expected), len(got), got)
			}

			for i, eg := range tt.expected {
				if got[i].Name != eg.Name {
					t.Errorf("group %d: expected name %q, got %q", i, eg.Name, got[i].Name)
				}
				if len(got[i].Paths) != len(eg.Paths) {
					t.Errorf("group %d (%s): expected %d paths, got %d", i, eg.Name, len(eg.Paths), len(got[i].Paths))
					continue
				}
				for j, ep := range eg.Paths {
					if got[i].Paths[j] != ep {
						t.Errorf("group %d (%s), path %d: expected %q, got %q", i, eg.Name, j, ep, got[i].Paths[j])
					}
				}
			}
		})
	}
}

// E-PENPAL-SRC-RP1: verifies ClassifyFile maps rp1 paths to correct types.
func TestClassifyRP1File(t *testing.T) {
	st := GetSourceType("rp1")
	if st == nil || st.ClassifyFile == nil {
		t.Fatal("rp1 source type not registered or has no ClassifyFile")
	}

	tests := []struct {
		path     string
		expected string
	}{
		// context
		{"context/index.md", "knowledge"},
		{"context/architecture.md", "knowledge"},
		// archives hidden
		{"work/archives/features/old/tasks.md", ""},
		{"work/archives/prds/old-prd/old-prd.md", ""},
		// worktrees and notes hidden
		{"work/worktrees/feature-branch/main.go", ""},
		{"work/notes/internal-note.md", ""},
		// feature files
		{"work/features/auth/requirements.md", "requirement"},
		{"work/features/auth/design.md", "design"},
		{"work/features/auth/design-decisions.md", "design"},
		{"work/features/auth/tasks.md", "task"},
		{"work/features/auth/field-notes.md", "field-notes"},
		{"work/features/auth/hypotheses.md", "hypothesis"},
		{"work/features/auth/test_report.md", "test-report"},
		{"work/features/auth/verification-report.md", "verification"},
		{"work/features/auth/unknown.md", "other"},
		// work subdirectories
		{"work/quick-builds/2025-01-build.md", "quick"},
		{"work/prds/my-prd.md", "prd"},
		{"work/research/2025-01-topic.md", "research"},
		{"work/pr-reviews/123/review.md", "review"},
		{"work/content/blog/post.md", "content"},
		{"work/issues/bug-42/investigation_report.md", "investigation"},
		{"work/issues/bug-42/root_cause_analysis.md", "analysis"},
		{"work/issues/bug-42/implementation_plan.md", "plan"},
		{"work/issues/bug-42/evidence/key_findings.md", "evidence"},
		{"work/issues/bug-42/unknown.md", "other"},
		// charter and reports
		{"work/charter.md", "charter"},
		{"work/audit-report.md", "report"},
		{"work/security-report.md", "report"},
		{"work/strategy-report.md", "report"},
		{"work/investigation-report.md", "report"},
		{"work/project-overview.md", "report"},
		// unknown top-level
		{"work/random.md", "other"},
		{"something-else.md", "other"},
	}

	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			got := st.ClassifyFile(tt.path)
			if got != tt.expected {
				t.Errorf("ClassifyFile(%q) = %q, want %q", tt.path, got, tt.expected)
			}
		})
	}
}

// E-PENPAL-DISCOVERY: verifies worktree deduplication keeps the main worktree.
func TestDeduplicateWorktreeProjects(t *testing.T) {
	mkWT := func(path, branch string, isMain bool) Worktree {
		return Worktree{Name: filepath.Base(path), Path: path, Branch: branch, IsMain: isMain}
	}

	tests := []struct {
		name      string
		projects  []Project
		wantNames []string
	}{
		{
			name: "no worktrees, no dedup",
			projects: []Project{
				{Name: "alpha", Path: "/ws/alpha"},
				{Name: "beta", Path: "/ws/beta"},
			},
			wantNames: []string{"alpha", "beta"},
		},
		{
			name: "worktree project removed, main kept",
			projects: []Project{
				{Name: "myrepo", Path: "/ws/myrepo", Worktrees: []Worktree{
					mkWT("/ws/myrepo", "main", true),
					mkWT("/ws/myrepo-wt", "feature", false),
				}},
				{Name: "myrepo-wt", Path: "/ws/myrepo-wt", Worktrees: []Worktree{
					mkWT("/ws/myrepo", "main", false),
					mkWT("/ws/myrepo-wt", "feature", true),
				}},
			},
			wantNames: []string{"myrepo"},
		},
		{
			name: "neither is main, first kept",
			projects: []Project{
				{Name: "wt-a", Path: "/ws/wt-a", Worktrees: []Worktree{
					mkWT("/elsewhere/repo", "main", false),
					mkWT("/ws/wt-a", "branch-a", true),
					mkWT("/ws/wt-b", "branch-b", false),
				}},
				{Name: "wt-b", Path: "/ws/wt-b", Worktrees: []Worktree{
					mkWT("/elsewhere/repo", "main", false),
					mkWT("/ws/wt-a", "branch-a", false),
					mkWT("/ws/wt-b", "branch-b", true),
				}},
			},
			wantNames: []string{"wt-a"},
		},
		{
			name: "main is second project, it wins",
			projects: []Project{
				// DiscoverWorktrees("/ws/feature-wt") sets IsMain relative
				// to feature-wt, NOT the repo's actual main worktree.
				{Name: "feature-wt", Path: "/ws/feature-wt", Worktrees: []Worktree{
					mkWT("/ws/mainrepo", "main", false),
					mkWT("/ws/feature-wt", "feature", true),
				}},
				{Name: "mainrepo", Path: "/ws/mainrepo", Worktrees: []Worktree{
					mkWT("/ws/mainrepo", "main", true),
					mkWT("/ws/feature-wt", "feature", false),
				}},
			},
			wantNames: []string{"mainrepo"},
		},
		{
			name: "unrelated projects not affected",
			projects: []Project{
				{Name: "alpha", Path: "/ws/alpha"},
				{Name: "repo", Path: "/ws/repo", Worktrees: []Worktree{
					mkWT("/ws/repo", "main", true),
					mkWT("/ws/repo-wt", "feat", false),
				}},
				{Name: "repo-wt", Path: "/ws/repo-wt", Worktrees: []Worktree{
					mkWT("/ws/repo", "main", false),
					mkWT("/ws/repo-wt", "feat", true),
				}},
				{Name: "beta", Path: "/ws/beta"},
			},
			wantNames: []string{"alpha", "repo", "beta"},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := deduplicateWorktreeProjects(tt.projects)
			if len(got) != len(tt.wantNames) {
				names := make([]string, len(got))
				for i, p := range got {
					names[i] = p.Name
				}
				t.Fatalf("got %d projects %v, want %d %v", len(got), names, len(tt.wantNames), tt.wantNames)
			}
			for i, name := range tt.wantNames {
				if got[i].Name != name {
					t.Errorf("project[%d].Name = %q, want %q", i, got[i].Name, name)
				}
			}
		})
	}
}

// E-PENPAL-SRC-ANCHORS: verifies ClassifyFile recognizes four content filenames and skips all others.
func TestClassifyAnchorsFile(t *testing.T) {
	st := GetSourceType("anchors")
	if st == nil || st.ClassifyFile == nil {
		t.Fatal("anchors source type not registered or has no ClassifyFile")
	}

	tests := []struct {
		path     string
		expected string
	}{
		// ANCHORS.md is skipped (module membership enforced by RequireSibling)
		{"ANCHORS.md", ""},
		{"auth/ANCHORS.md", ""},
		// content files
		{"PRODUCT.md", "product"},
		{"ERD.md", "engineering"},
		{"TESTING.md", "testing"},
		{"DEPENDENCIES.md", "dependencies"},
		// nested module content files
		{"auth/PRODUCT.md", "product"},
		{"auth/ERD.md", "engineering"},
		{"services/payments/TESTING.md", "testing"},
		// non-ANCHORS files are skipped
		{"README.md", ""},
		{"docs/guide.md", ""},
		{"src/main.go", ""},
		{"auth/design.md", ""},
	}

	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			got := st.ClassifyFile(tt.path)
			if got != tt.expected {
				t.Errorf("ClassifyFile(%q) = %q, want %q", tt.path, got, tt.expected)
			}
		})
	}
}

// E-PENPAL-SRC-ANCHORS: verifies GroupFiles groups by module directory with canonical ordering.
// Note: ANCHORS.md never reaches GroupFiles — it's filtered by ClassifyFile (returns "")
// at scan time. Module membership is enforced by RequireSibling at scan time, so
// GroupFiles receives only pre-validated content files.
func TestGroupAnchorsPaths(t *testing.T) {
	st := GetSourceType("anchors")
	if st == nil || st.GroupFiles == nil {
		t.Fatal("anchors source type not registered or has no GroupFiles")
	}

	tests := []struct {
		name     string
		paths    []string
		expected []FileGroup
	}{
		{
			name:     "empty input produces no groups",
			paths:    nil,
			expected: nil,
		},
		{
			name: "single root module",
			paths: []string{
				"PRODUCT.md",
				"ERD.md",
			},
			expected: []FileGroup{
				{Name: "(root)", Paths: []string{"PRODUCT.md", "ERD.md"}},
			},
		},
		{
			name: "root module file ordering",
			paths: []string{
				"DEPENDENCIES.md",
				"PRODUCT.md",
				"TESTING.md",
				"ERD.md",
			},
			expected: []FileGroup{
				{Name: "(root)", Paths: []string{
					"PRODUCT.md", "ERD.md", "TESTING.md", "DEPENDENCIES.md",
				}},
			},
		},
		{
			name: "nested modules sorted alphabetically",
			paths: []string{
				"payments/PRODUCT.md",
				"auth/PRODUCT.md",
				"auth/ERD.md",
			},
			expected: []FileGroup{
				{Name: "auth", Paths: []string{"auth/PRODUCT.md", "auth/ERD.md"}},
				{Name: "payments", Paths: []string{"payments/PRODUCT.md"}},
			},
		},
		{
			name: "root and nested modules together",
			paths: []string{
				"PRODUCT.md",
				"services/auth/ERD.md",
			},
			expected: []FileGroup{
				{Name: "(root)", Paths: []string{"PRODUCT.md"}},
				{Name: "services/auth", Paths: []string{"services/auth/ERD.md"}},
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := st.GroupFiles(tt.paths)

			if len(got) != len(tt.expected) {
				t.Fatalf("expected %d groups, got %d: %+v", len(tt.expected), len(got), got)
			}

			for i, eg := range tt.expected {
				if got[i].Name != eg.Name {
					t.Errorf("group %d: expected name %q, got %q", i, eg.Name, got[i].Name)
				}
				if len(got[i].Paths) != len(eg.Paths) {
					t.Errorf("group %d (%s): expected %d paths, got %d: %v", i, eg.Name, len(eg.Paths), len(got[i].Paths), got[i].Paths)
					continue
				}
				for j, ep := range eg.Paths {
					if got[i].Paths[j] != ep {
						t.Errorf("group %d (%s), path %d: expected %q, got %q", i, eg.Name, j, ep, got[i].Paths[j])
					}
				}
			}
		})
	}
}

// E-PENPAL-SRC-ANCHORS: verifies that a marker-only module (directory with ANCHORS.md
// but no content files) produces no groups. This documents the intended behavior:
// RequireSibling admits the directory, ClassifyFile skips ANCHORS.md, and
// GroupFiles receives an empty slice.
func TestGroupAnchorsPaths_MarkerOnlyModule(t *testing.T) {
	st := GetSourceType("anchors")
	if st == nil || st.GroupFiles == nil {
		t.Fatal("anchors source type not registered or has no GroupFiles")
	}

	// Simulate what the scanner produces for a directory containing only ANCHORS.md:
	// ClassifyFile returns "" for ANCHORS.md, so it never reaches GroupFiles.
	got := st.GroupFiles(nil)
	if len(got) != 0 {
		t.Errorf("expected 0 groups for marker-only module, got %d: %+v", len(got), got)
	}
}

// E-PENPAL-SRC-ANCHORS: verifies canonical file ordering is stable for all recognized
// content filenames. An unrecognized filename would get Go's zero-value (0) from the
// map lookup, colliding with PRODUCT.md's position — RequireSibling and ClassifyFile
// prevent this, but this test documents the sort contract.
func TestAnchorsFileOrder(t *testing.T) {
	// Every content filename must have a unique position in the order map.
	contentFiles := []string{"PRODUCT.md", "ERD.md", "TESTING.md", "DEPENDENCIES.md"}
	seen := map[int]string{}
	for _, f := range contentFiles {
		pos, ok := anchorsFileOrder[f]
		if !ok {
			t.Errorf("anchorsFileOrder missing entry for %q", f)
			continue
		}
		if prev, dup := seen[pos]; dup {
			t.Errorf("anchorsFileOrder position %d shared by %q and %q", pos, prev, f)
		}
		seen[pos] = f
	}

	// ANCHORS.md must NOT be in the order map (it never reaches GroupFiles).
	if _, ok := anchorsFileOrder["ANCHORS.md"]; ok {
		t.Error("anchorsFileOrder should not contain ANCHORS.md")
	}
}

// E-PENPAL-SRC-ANCHORS: verifies RequireSibling is set on the anchors source type.
func TestAnchorsRequireSibling(t *testing.T) {
	st := GetSourceType("anchors")
	if st == nil {
		t.Fatal("anchors source type not registered")
	}
	if st.RequireSibling != "ANCHORS.md" {
		t.Errorf("RequireSibling = %q, want %q", st.RequireSibling, "ANCHORS.md")
	}
}

// E-PENPAL-SRC-CLAUDE-PLANS: verifies the claude-plans source type is registered with correct properties.
func TestClaudePlansSourceType(t *testing.T) {
	st := GetSourceType("claude-plans")
	if st == nil {
		t.Fatal("claude-plans source type not registered")
	}

	// Verify Name
	if st.Name != "claude-plans" {
		t.Errorf("Name = %q, want %q", st.Name, "claude-plans")
	}

	// Verify ClassifyFile returns "plan" for any .md file
	if st.ClassifyFile == nil {
		t.Fatal("ClassifyFile should not be nil")
	}
	testPaths := []string{
		"some-plan.md",
		"deeply/nested/file.md",
		"anything.md",
		"not-even-markdown.txt",
		"",
	}
	for _, p := range testPaths {
		got := st.ClassifyFile(p)
		if got != "plan" {
			t.Errorf("ClassifyFile(%q) = %q, want %q", p, got, "plan")
		}
	}

	// Verify GroupFiles is nil
	if st.GroupFiles != nil {
		t.Error("GroupFiles should be nil for claude-plans source type")
	}
}

// E-PENPAL-CLAUDE-PLANS-DETECT: verifies DiscoverClaudePlans returns a synthetic project
// when ~/.claude/plans/ contains .md files.
func TestDiscoverClaudePlans(t *testing.T) {
	// Create a temp directory to mimic ~/.claude/plans/
	tmpDir := t.TempDir()
	plansDir := filepath.Join(tmpDir, ".claude", "plans")
	if err := os.MkdirAll(plansDir, 0755); err != nil {
		t.Fatalf("failed to create plans dir: %v", err)
	}

	// Write some .md files
	os.WriteFile(filepath.Join(plansDir, "plan-a.md"), []byte("# Plan A"), 0644)
	os.WriteFile(filepath.Join(plansDir, "plan-b.md"), []byte("# Plan B"), 0644)

	// DiscoverClaudePlans uses os.UserHomeDir() which we can't easily override,
	// so we test the helper countMdFiles and the structure of the returned project
	// by verifying countMdFiles works correctly.
	count := countMdFiles(plansDir)
	if count != 2 {
		t.Errorf("countMdFiles() = %d, want 2", count)
	}

	// Verify countMdFiles returns 0 for an empty directory
	emptyDir := filepath.Join(tmpDir, "empty")
	os.MkdirAll(emptyDir, 0755)
	if c := countMdFiles(emptyDir); c != 0 {
		t.Errorf("countMdFiles(empty) = %d, want 0", c)
	}

	// Verify countMdFiles returns 0 for a nonexistent directory
	if c := countMdFiles(filepath.Join(tmpDir, "nonexistent")); c != 0 {
		t.Errorf("countMdFiles(nonexistent) = %d, want 0", c)
	}

	// Test the actual DiscoverClaudePlans function.
	// We can't control os.UserHomeDir(), but we can verify the function
	// returns a well-formed project when it does find plans.
	project, found := DiscoverClaudePlans()
	if found {
		// If the user running tests happens to have ~/.claude/plans/ with .md files,
		// verify the project structure is correct.
		if project.Name != ".claude/plans" {
			t.Errorf("project.Name = %q, want %q", project.Name, ".claude/plans")
		}
		if project.Origin != "standalone" {
			t.Errorf("project.Origin = %q, want %q", project.Origin, "standalone")
		}
		if len(project.Sources) != 1 {
			t.Fatalf("expected 1 source, got %d", len(project.Sources))
		}
		src := project.Sources[0]
		if src.Name != "plans" {
			t.Errorf("source.Name = %q, want %q", src.Name, "plans")
		}
		if src.Type != "tree" {
			t.Errorf("source.Type = %q, want %q", src.Type, "tree")
		}
		if src.SourceTypeName != "claude-plans" {
			t.Errorf("source.SourceTypeName = %q, want %q", src.SourceTypeName, "claude-plans")
		}
		if !src.Auto {
			t.Error("source.Auto should be true")
		}
	}
	// If not found, that's also valid — the test environment may not have ~/.claude/plans/
}

// E-PENPAL-CLAUDE-PLANS-DETECT: verifies countMdFiles walks subdirectories.
func TestCountMdFilesNested(t *testing.T) {
	tmpDir := t.TempDir()
	os.MkdirAll(filepath.Join(tmpDir, "sub"), 0755)
	os.WriteFile(filepath.Join(tmpDir, "top.md"), []byte("# Top"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "sub", "nested.md"), []byte("# Nested"), 0644)
	os.WriteFile(filepath.Join(tmpDir, "not-md.txt"), []byte("text"), 0644)

	count := countMdFiles(tmpDir)
	if count != 2 {
		t.Errorf("countMdFiles() = %d, want 2", count)
	}
}

func BenchmarkGroupRP1Paths(b *testing.B) {
	st := GetSourceType("rp1")
	if st == nil || st.GroupFiles == nil {
		b.Fatal("rp1 source type not registered or has no GroupFiles")
	}

	paths := make([]string, 60)
	for i := 0; i < 10; i++ {
		paths[i] = "context/file-" + string(rune('a'+i)) + ".md"
	}
	for i := 10; i < 60; i++ {
		paths[i] = "work/features/feature-" + string(rune('a'+i)) + "/requirements.md"
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		st.GroupFiles(paths)
	}
}
