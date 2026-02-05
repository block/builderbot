package discovery

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

type GitInfo struct {
	Branch string
	Dirty  bool
}

func GetGitInfo(projectPath string) *GitInfo {
	// Read branch directly from .git/HEAD — avoids 2 subprocess calls
	headContent, err := os.ReadFile(filepath.Join(projectPath, ".git", "HEAD"))
	if err != nil {
		return nil // not a git repo (or bare repo)
	}

	info := &GitInfo{}
	head := strings.TrimSpace(string(headContent))
	if strings.HasPrefix(head, "ref: refs/heads/") {
		info.Branch = strings.TrimPrefix(head, "ref: refs/heads/")
	} else if len(head) >= 7 {
		info.Branch = head[:7] // detached HEAD, show short hash
	}

	// Still need git subprocess for dirty check (no good file-based alternative)
	cmd := exec.Command("git", "-C", projectPath, "status", "--porcelain")
	if out, err := cmd.Output(); err == nil {
		info.Dirty = len(out) > 0
	}

	return info
}
