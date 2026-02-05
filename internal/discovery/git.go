package discovery

import (
	"os/exec"
	"strings"
)

type GitInfo struct {
	Branch string
	Dirty  bool
}

func GetGitInfo(projectPath string) *GitInfo {
	// Check if it's a git repo
	cmd := exec.Command("git", "-C", projectPath, "rev-parse", "--git-dir")
	if err := cmd.Run(); err != nil {
		return nil
	}

	info := &GitInfo{}

	// Get branch name
	cmd = exec.Command("git", "-C", projectPath, "rev-parse", "--abbrev-ref", "HEAD")
	if out, err := cmd.Output(); err == nil {
		info.Branch = strings.TrimSpace(string(out))
	}

	// Check if dirty
	cmd = exec.Command("git", "-C", projectPath, "status", "--porcelain")
	if out, err := cmd.Output(); err == nil {
		info.Dirty = len(out) > 0
	}

	return info
}
