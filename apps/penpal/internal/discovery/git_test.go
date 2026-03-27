package discovery

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// E-PENPAL-GIT-ENRICH: verifies GetGitInfo returns nil for a nonexistent path.
func TestGetGitInfo_NonexistentPath(t *testing.T) {
	info := GetGitInfo("/nonexistent/path/that/does/not/exist")
	if info != nil {
		t.Errorf("GetGitInfo on nonexistent path returned %+v, want nil", info)
	}
}

// E-PENPAL-GIT-ENRICH: verifies GetGitInfo returns nil for a directory that is not a git repo.
func TestGetGitInfo_NotAGitRepo(t *testing.T) {
	tmpDir := t.TempDir()
	// Create a regular directory with no .git
	info := GetGitInfo(tmpDir)
	if info != nil {
		t.Errorf("GetGitInfo on non-git directory returned %+v, want nil", info)
	}
}

// E-PENPAL-GIT-ENRICH: verifies enrichGitInfo does not panic on a nonexistent path.
func TestEnrichGitInfo_NonexistentPath(t *testing.T) {
	info := &GitInfo{Branch: "main"}
	// enrichGitInfo calls git subprocess; on a nonexistent path it should
	// gracefully handle the failure (no panic, returns the info struct).
	result := enrichGitInfo(info, "/nonexistent/path/that/does/not/exist")
	if result == nil {
		t.Fatal("enrichGitInfo returned nil, expected non-nil GitInfo")
	}
	if result.Branch != "main" {
		t.Errorf("Branch = %q, want %q", result.Branch, "main")
	}
	// Dirty should be false since git status failed (or returned empty)
	// The function should not have panicked — that's the main assertion.
}

// E-PENPAL-GIT-ENRICH: verifies enrichGitInfo returns sane defaults for a non-git directory.
func TestEnrichGitInfo_NonGitDir(t *testing.T) {
	tmpDir := t.TempDir()
	info := &GitInfo{Branch: "feature"}
	result := enrichGitInfo(info, tmpDir)
	if result == nil {
		t.Fatal("enrichGitInfo returned nil, expected non-nil GitInfo")
	}
	if result.Branch != "feature" {
		t.Errorf("Branch = %q, want %q", result.Branch, "feature")
	}
	// UnpushedCommitTime should be zero since git log fails
	if !result.UnpushedCommitTime.IsZero() {
		t.Errorf("UnpushedCommitTime = %v, want zero", result.UnpushedCommitTime)
	}
}

// E-PENPAL-GIT-ENRICH: verifies parseUnstagedModTime extracts mod times from porcelain output.
func TestParseUnstagedModTime(t *testing.T) {
	tmpDir := t.TempDir()

	// Create files with known mod times
	file1 := filepath.Join(tmpDir, "file1.go")
	file2 := filepath.Join(tmpDir, "file2.go")
	os.WriteFile(file1, []byte("package main"), 0644)
	os.WriteFile(file2, []byte("package main"), 0644)

	// Set file2 to be newer
	oldTime := time.Now().Add(-1 * time.Hour)
	os.Chtimes(file1, oldTime, oldTime)

	// Simulate git status --porcelain output
	porcelain := " M file1.go\n M file2.go"
	modTime := parseUnstagedModTime(tmpDir, porcelain)

	// Should return the more recent file's mod time (file2)
	file2Info, _ := os.Stat(file2)
	if !modTime.Equal(file2Info.ModTime()) {
		t.Errorf("parseUnstagedModTime returned %v, want %v", modTime, file2Info.ModTime())
	}
}

// E-PENPAL-GIT-ENRICH: verifies parseUnstagedModTime handles empty output gracefully.
func TestParseUnstagedModTime_Empty(t *testing.T) {
	tmpDir := t.TempDir()
	modTime := parseUnstagedModTime(tmpDir, "")
	if !modTime.IsZero() {
		t.Errorf("parseUnstagedModTime on empty output = %v, want zero", modTime)
	}
}

// E-PENPAL-GIT-ENRICH: verifies parseUnstagedModTime handles renamed files (->).
func TestParseUnstagedModTime_RenamedFile(t *testing.T) {
	tmpDir := t.TempDir()
	newFile := filepath.Join(tmpDir, "new-name.go")
	os.WriteFile(newFile, []byte("package main"), 0644)

	// Git porcelain shows renames as "R  old-name.go -> new-name.go"
	porcelain := "R  old-name.go -> new-name.go"
	modTime := parseUnstagedModTime(tmpDir, porcelain)

	newInfo, _ := os.Stat(newFile)
	if !modTime.Equal(newInfo.ModTime()) {
		t.Errorf("parseUnstagedModTime for rename = %v, want %v", modTime, newInfo.ModTime())
	}
}

// E-PENPAL-GIT-ENRICH: verifies parseUnstagedModTime handles files that don't exist on disk.
func TestParseUnstagedModTime_MissingFiles(t *testing.T) {
	tmpDir := t.TempDir()
	// References a file that doesn't exist — should not panic
	porcelain := " D deleted-file.go"
	modTime := parseUnstagedModTime(tmpDir, porcelain)
	if !modTime.IsZero() {
		t.Errorf("parseUnstagedModTime for deleted file = %v, want zero", modTime)
	}
}
