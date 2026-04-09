package discovery

import (
	"os"
	"os/exec"
	"path/filepath"
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

// initGitRepo creates a git repo in dir with an initial commit.
func initGitRepo(t *testing.T, dir string) {
	t.Helper()
	for _, args := range [][]string{
		{"init"},
		{"config", "user.email", "test@test.com"},
		{"config", "user.name", "Test"},
		{"commit", "--allow-empty", "-m", "init"},
	} {
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
}

// resolveSymlinks resolves symlinks in a path for reliable comparison on macOS
// where /var → /private/var.
func resolveSymlinks(t *testing.T, path string) string {
	t.Helper()
	resolved, err := filepath.EvalSymlinks(path)
	if err != nil {
		t.Fatalf("EvalSymlinks(%q): %v", path, err)
	}
	return resolved
}

// E-PENPAL-WORKTREE-WATCH: verifies gitWorktreesDir returns the .git/worktrees/ dir for a repo with worktrees.
func TestWorktreesDir_MainWorktree(t *testing.T) {
	mainDir := resolveSymlinks(t, t.TempDir())
	initGitRepo(t, mainDir)

	// Before adding a worktree, the dir doesn't exist
	if got := gitWorktreesDir(mainDir); got != "" {
		t.Fatalf("expected empty before worktree add, got %q", got)
	}

	// Add a worktree
	wtDir := filepath.Join(resolveSymlinks(t, t.TempDir()), "my-worktree")
	cmd := exec.Command("git", "-C", mainDir, "worktree", "add", "-b", "test-branch", wtDir)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git worktree add: %v\n%s", err, out)
	}

	// Now gitWorktreesDir should return the .git/worktrees/ path
	got := gitWorktreesDir(mainDir)
	want := filepath.Join(mainDir, ".git", "worktrees")
	if got != want {
		t.Errorf("gitWorktreesDir(main) = %q, want %q", got, want)
	}

	// It should also work when called from the linked worktree
	got2 := gitWorktreesDir(wtDir)
	if got2 != want {
		t.Errorf("gitWorktreesDir(linked) = %q, want %q", got2, want)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies gitWorktreesDir returns "" for a non-git directory.
func TestWorktreesDir_NotGitRepo(t *testing.T) {
	dir := t.TempDir()
	if got := gitWorktreesDir(dir); got != "" {
		t.Errorf("gitWorktreesDir(non-git) = %q, want empty", got)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies gitWorktreesDir returns "" for a repo with no worktrees.
func TestWorktreesDir_NoWorktrees(t *testing.T) {
	dir := t.TempDir()
	initGitRepo(t, dir)
	if got := gitWorktreesDir(dir); got != "" {
		t.Errorf("gitWorktreesDir(no worktrees) = %q, want empty", got)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies worktree directory appears after git worktree add
// and disappears after git worktree remove.
func TestWorktreesDir_AddRemoveCycle(t *testing.T) {
	mainDir := resolveSymlinks(t, t.TempDir())
	initGitRepo(t, mainDir)

	wtPath := filepath.Join(resolveSymlinks(t, t.TempDir()), "wt")
	cmd := exec.Command("git", "-C", mainDir, "worktree", "add", "-b", "wt-branch", wtPath)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git worktree add: %v\n%s", err, out)
	}

	wtDir := gitWorktreesDir(mainDir)
	if wtDir == "" {
		t.Fatal("expected non-empty after add")
	}

	// Verify the specific worktree entry exists
	entries, err := os.ReadDir(wtDir)
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, e := range entries {
		if e.Name() == filepath.Base(wtPath) {
			found = true
		}
	}
	if !found {
		t.Errorf("expected entry %q in %s", filepath.Base(wtPath), wtDir)
	}

	// Remove the worktree
	cmd = exec.Command("git", "-C", mainDir, "worktree", "remove", wtPath)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git worktree remove: %v\n%s", err, out)
	}

	// After removing the last worktree, the worktrees/ dir should be gone
	if got := gitWorktreesDir(mainDir); got != "" {
		t.Errorf("expected empty after removing last worktree, got %q", got)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies gitCommonDirFS returns "" for malformed .git file.
func TestGitCommonDirFS_MalformedGitFile(t *testing.T) {
	dir := t.TempDir()
	// .git file with no "gitdir:" prefix
	os.WriteFile(filepath.Join(dir, ".git"), []byte("not a gitdir line\n"), 0o644)
	if got := gitCommonDirFS(dir); got != "" {
		t.Errorf("expected empty for malformed .git file, got %q", got)
	}
}

// E-PENPAL-WORKTREE-WATCH: verifies gitCommonDirFS returns "" when commondir file is missing.
func TestGitCommonDirFS_MissingCommondir(t *testing.T) {
	dir := t.TempDir()
	gitDir := filepath.Join(dir, "fake-gitdir")
	os.MkdirAll(gitDir, 0o755)
	// .git file points to a valid directory but commondir file doesn't exist
	os.WriteFile(filepath.Join(dir, ".git"), []byte("gitdir: "+gitDir+"\n"), 0o644)
	if got := gitCommonDirFS(dir); got != "" {
		t.Errorf("expected empty for missing commondir, got %q", got)
	}
}
