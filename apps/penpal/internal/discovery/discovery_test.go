package discovery

import (
	"path/filepath"
	"testing"
)

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
