package discovery

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

type GitInfo struct {
	Branch             string
	Dirty              bool
	UnstagedModTime    time.Time // most recent mod time among unstaged changed files
	UnpushedCommitTime time.Time // most recent unpushed commit time
}

func GetGitInfo(projectPath string) *GitInfo {
	// Read branch directly from .git/HEAD — avoids 2 subprocess calls
	// For worktrees, .git is a file containing "gitdir: ...", so we
	// fall back to git rev-parse if the direct file read fails.
	headContent, err := os.ReadFile(filepath.Join(projectPath, ".git", "HEAD"))
	if err != nil {
		// Check if .git is a file (worktree) rather than a directory
		gitFile := filepath.Join(projectPath, ".git")
		if fi, statErr := os.Stat(gitFile); statErr == nil && !fi.IsDir() {
			// It's a worktree — use git rev-parse to get branch
			cmd := exec.Command("git", "-C", projectPath, "rev-parse", "--abbrev-ref", "HEAD")
			out, cmdErr := cmd.Output()
			if cmdErr != nil {
				return nil
			}
			info := &GitInfo{Branch: strings.TrimSpace(string(out))}
			return enrichGitInfo(info, projectPath)
		}
		return nil // not a git repo (or bare repo)
	}

	info := &GitInfo{}
	head := strings.TrimSpace(string(headContent))
	if strings.HasPrefix(head, "ref: refs/heads/") {
		info.Branch = strings.TrimPrefix(head, "ref: refs/heads/")
	} else if len(head) >= 7 {
		info.Branch = head[:7] // detached HEAD, show short hash
	}

	return enrichGitInfo(info, projectPath)
}

// enrichGitInfo adds dirty status and unpushed commit info to a GitInfo.
func enrichGitInfo(info *GitInfo, projectPath string) *GitInfo {
	// Still need git subprocess for dirty check (no good file-based alternative)
	cmd := exec.Command("git", "-C", projectPath, "status", "--porcelain")
	if out, err := cmd.Output(); err == nil {
		info.Dirty = len(out) > 0
		if info.Dirty {
			info.UnstagedModTime = parseUnstagedModTime(projectPath, string(out))
		}
	}

	// Most recent unpushed commit time
	cmd2 := exec.Command("git", "-C", projectPath, "log", "@{upstream}..HEAD", "--format=%cI", "-1")
	if out2, err := cmd2.Output(); err == nil {
		if ts := strings.TrimSpace(string(out2)); ts != "" {
			if t, err := time.Parse(time.RFC3339, ts); err == nil {
				info.UnpushedCommitTime = t
			}
		}
	}

	return info
}

// parseUnstagedModTime finds the most recent modification time among files
// listed in git status --porcelain output.
func parseUnstagedModTime(projectPath, porcelainOutput string) time.Time {
	var latest time.Time
	for _, line := range strings.Split(strings.TrimSpace(porcelainOutput), "\n") {
		if len(line) < 4 {
			continue
		}
		path := line[3:]
		if idx := strings.Index(path, " -> "); idx >= 0 {
			path = path[idx+4:]
		}
		path = strings.Trim(path, "\"")
		if fi, err := os.Stat(filepath.Join(projectPath, path)); err == nil {
			if fi.ModTime().After(latest) {
				latest = fi.ModTime()
			}
		}
	}
	return latest
}
