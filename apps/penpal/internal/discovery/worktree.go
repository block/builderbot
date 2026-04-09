package discovery

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// gitCommonDirFS resolves the shared .git directory using only filesystem
// reads — no subprocess. For a main worktree .git is a directory; for a
// linked worktree .git is a file containing "gitdir: <path>" and the
// referenced gitdir contains a "commondir" file pointing back to the
// shared .git.
func gitCommonDirFS(projectPath string) string {
	gitPath := filepath.Join(projectPath, ".git")
	info, err := os.Lstat(gitPath)
	if err != nil {
		return ""
	}
	// Main worktree: .git is a directory — it IS the common dir.
	if info.IsDir() {
		return gitPath
	}
	// Linked worktree: .git is a file with "gitdir: <path>".
	data, err := os.ReadFile(gitPath)
	if err != nil {
		return ""
	}
	line := strings.TrimSpace(string(data))
	if !strings.HasPrefix(line, "gitdir: ") {
		return ""
	}
	gitDir := strings.TrimPrefix(line, "gitdir: ")
	if !filepath.IsAbs(gitDir) {
		gitDir = filepath.Join(projectPath, gitDir)
	}
	gitDir = filepath.Clean(gitDir)
	// Read commondir file to find the shared .git directory.
	cdPath := filepath.Join(gitDir, "commondir")
	cdData, err := os.ReadFile(cdPath)
	if err != nil {
		return ""
	}
	commonDir := strings.TrimSpace(string(cdData))
	if !filepath.IsAbs(commonDir) {
		commonDir = filepath.Join(gitDir, commonDir)
	}
	return filepath.Clean(commonDir)
}

// Worktree represents a git worktree associated with a project.
type Worktree struct {
	Name   string `json:"name"`   // directory name (e.g., "fancy-name")
	Path   string `json:"path"`   // absolute filesystem path
	Branch string `json:"branch"` // checked-out branch
	IsMain bool   `json:"isMain"` // true for the original clone
}

// DiscoverWorktrees returns all git worktrees for the project at projectPath.
// The main worktree is always first. Returns nil if the project is not a git repo
// or has no additional worktrees.
// E-PENPAL-WORKTREE-DISCOVERY: runs git worktree list --porcelain and parses the output.
func DiscoverWorktrees(projectPath string) []Worktree {
	cmd := exec.Command("git", "-C", projectPath, "worktree", "list", "--porcelain")
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	return parseWorktreeList(projectPath, string(out))
}

// parseWorktreeList parses `git worktree list --porcelain` output into Worktree structs.
// E-PENPAL-WORKTREE-DISCOVERY: parses porcelain output, strips refs/heads/ prefix, sets IsMain flag.
func parseWorktreeList(projectPath string, output string) []Worktree {
	if output == "" {
		return nil
	}

	var worktrees []Worktree
	var current *Worktree
	mainPath := filepath.Clean(projectPath)

	for _, line := range strings.Split(output, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			if current != nil {
				worktrees = append(worktrees, *current)
				current = nil
			}
			continue
		}

		if strings.HasPrefix(line, "worktree ") {
			wtPath := strings.TrimPrefix(line, "worktree ")
			cleanPath := filepath.Clean(wtPath)
			current = &Worktree{
				Path:   cleanPath,
				Name:   filepath.Base(cleanPath),
				IsMain: cleanPath == mainPath,
			}
		} else if strings.HasPrefix(line, "branch ") {
			if current != nil {
				branch := strings.TrimPrefix(line, "branch ")
				// Strip refs/heads/ prefix
				branch = strings.TrimPrefix(branch, "refs/heads/")
				current.Branch = branch
			}
		} else if line == "bare" {
			// Skip bare repos
			current = nil
		}
	}
	// Handle last entry without trailing newline
	if current != nil {
		worktrees = append(worktrees, *current)
	}

	// Only return if there are additional worktrees beyond main
	if len(worktrees) <= 1 {
		return nil
	}

	return worktrees
}

// gitWorktreesDir returns the path to the .git/worktrees/ directory for the
// repository that projectPath belongs to, or "" if it doesn't exist.
// Uses pure filesystem reads via gitCommonDirFS — no subprocess calls.
func gitWorktreesDir(projectPath string) string {
	commonDir := gitCommonDirFS(projectPath)
	if commonDir == "" {
		return ""
	}
	wtDir := filepath.Join(commonDir, "worktrees")
	if info, err := os.Stat(wtDir); err == nil && info.IsDir() {
		return wtDir
	}
	return ""
}
