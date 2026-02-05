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

type GitCommit struct {
	Hash    string
	Message string
}

type GitChange struct {
	Path   string
	Status string // "new", "deleted", "modified"
}

type GitStatus struct {
	Commits []GitCommit
	Changes []GitChange
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

// GetGitStatus returns unpushed commits and uncommitted changes for a project.
func GetGitStatus(projectPath string) *GitStatus {
	cmd := exec.Command("git", "-C", projectPath, "rev-parse", "--git-dir")
	if err := cmd.Run(); err != nil {
		return nil
	}

	result := &GitStatus{}

	// Unpushed commits (ahead of upstream tracking branch)
	cmd = exec.Command("git", "-C", projectPath, "log", "@{u}..HEAD", "--oneline")
	if out, err := cmd.Output(); err == nil {
		for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
			if line == "" {
				continue
			}
			parts := strings.SplitN(line, " ", 2)
			commit := GitCommit{Hash: parts[0]}
			if len(parts) > 1 {
				commit.Message = parts[1]
			}
			result.Commits = append(result.Commits, commit)
		}
	}

	// Uncommitted changes
	cmd = exec.Command("git", "-C", projectPath, "status", "--porcelain")
	if out, err := cmd.Output(); err == nil {
		for _, line := range strings.Split(string(out), "\n") {
			if len(line) < 3 {
				continue
			}
			xy := line[:2]
			path := line[3:]

			var changeStatus string
			switch {
			case xy == "??" || xy[0] == 'A':
				changeStatus = "new"
			case xy[1] == 'D' || xy[0] == 'D':
				changeStatus = "deleted"
			default:
				changeStatus = "modified"
			}

			result.Changes = append(result.Changes, GitChange{Path: path, Status: changeStatus})
		}
	}

	return result
}
