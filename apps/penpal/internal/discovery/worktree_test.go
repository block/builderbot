package discovery

import (
	"testing"
)

// E-PENPAL-WORKTREE-DISCOVERY: verifies porcelain output parsing, IsMain flag, and bare repo skipping.
func TestParseWorktreeList(t *testing.T) {
	tests := []struct {
		name        string
		projectPath string
		output      string
		wantLen     int
		wantNames   []string
	}{
		{
			name:        "empty output",
			projectPath: "/repo",
			output:      "",
			wantLen:     0,
		},
		{
			name:        "single worktree (main only)",
			projectPath: "/repo",
			output:      "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\n",
			wantLen:     0, // returns nil when only main exists
		},
		{
			name:        "main plus one worktree",
			projectPath: "/repo",
			output:      "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /repo/.claude/worktrees/fancy-name\nHEAD def456\nbranch refs/heads/feature-branch\n\n",
			wantLen:     2,
			wantNames:   []string{"repo", "fancy-name"},
		},
		{
			name:        "main plus multiple worktrees",
			projectPath: "/home/user/project",
			output:      "worktree /home/user/project\nHEAD abc123\nbranch refs/heads/main\n\nworktree /home/user/project/.claude/worktrees/wt-a\nHEAD def456\nbranch refs/heads/branch-a\n\nworktree /home/user/project/.claude/worktrees/wt-b\nHEAD 789012\nbranch refs/heads/branch-b\n\n",
			wantLen:     3,
			wantNames:   []string{"project", "wt-a", "wt-b"},
		},
		{
			name:        "strips refs/heads/ prefix",
			projectPath: "/repo",
			output:      "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /tmp/wt\nHEAD def\nbranch refs/heads/my-feature\n\n",
			wantLen:     2,
		},
		{
			name:        "bare repo entry is skipped",
			projectPath: "/repo",
			output:      "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /bare\nbare\n\nworktree /tmp/wt\nHEAD def\nbranch refs/heads/feature\n\n",
			wantLen:     2,
			wantNames:   []string{"repo", "wt"},
		},
		{
			name:        "no trailing newline",
			projectPath: "/repo",
			output:      "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /tmp/wt\nHEAD def\nbranch refs/heads/feat",
			wantLen:     2,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := parseWorktreeList(tt.projectPath, tt.output)
			if len(got) != tt.wantLen {
				t.Fatalf("parseWorktreeList: got %d worktrees, want %d: %+v", len(got), tt.wantLen, got)
			}
			if tt.wantNames != nil {
				for i, name := range tt.wantNames {
					if got[i].Name != name {
						t.Errorf("worktree[%d].Name = %q, want %q", i, got[i].Name, name)
					}
				}
			}
			// Verify IsMain is set correctly
			for _, wt := range got {
				if wt.Path == tt.projectPath && !wt.IsMain {
					t.Errorf("worktree at project path should be IsMain=true")
				}
				if wt.Path != tt.projectPath && wt.IsMain {
					t.Errorf("worktree at %s should be IsMain=false", wt.Path)
				}
			}
		})
	}
}

// E-PENPAL-WORKTREE-DISCOVERY: verifies refs/heads/ prefix is stripped from branch names.
func TestParseWorktreeList_BranchStripping(t *testing.T) {
	output := "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /tmp/wt\nHEAD def\nbranch refs/heads/feature/nested\n\n"
	got := parseWorktreeList("/repo", output)
	if len(got) != 2 {
		t.Fatalf("expected 2 worktrees, got %d", len(got))
	}
	if got[0].Branch != "main" {
		t.Errorf("main branch = %q, want %q", got[0].Branch, "main")
	}
	if got[1].Branch != "feature/nested" {
		t.Errorf("wt branch = %q, want %q", got[1].Branch, "feature/nested")
	}
}
