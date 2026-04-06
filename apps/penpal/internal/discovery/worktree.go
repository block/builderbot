package discovery

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

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

// ResolveWorktree finds the worktree that contains the given absolute path.
// Returns the worktree name and the main project path, or empty strings if
// the path doesn't belong to any worktree.
func ResolveWorktree(projectPath string, absPath string) (worktreeName string, mainProjectPath string) {
	absPath = filepath.Clean(absPath)

	// First check if this path is inside the main project
	mainPath := filepath.Clean(projectPath)
	if strings.HasPrefix(absPath, mainPath+"/") || absPath == mainPath {
		// Check if it's inside a worktree subdirectory
		worktrees := DiscoverWorktrees(projectPath)
		for _, wt := range worktrees {
			if !wt.IsMain && (strings.HasPrefix(absPath, wt.Path+"/") || absPath == wt.Path) {
				return wt.Name, mainPath
			}
		}
		return "", mainPath
	}

	return "", ""
}

// FindMainWorktree returns the path to the main worktree for a given path
// that might be inside a worktree. It reads the .git file to find the
// gitdir and traces back to the main worktree.
func FindMainWorktree(path string) string {
	cmd := exec.Command("git", "-C", path, "rev-parse", "--git-common-dir")
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	commonDir := strings.TrimSpace(string(out))
	if commonDir == "" || commonDir == "." {
		return ""
	}

	// commonDir is the .git directory of the main worktree
	// If it's relative, resolve it relative to the path
	if !filepath.IsAbs(commonDir) {
		// Get the actual git dir for this worktree first
		cmd2 := exec.Command("git", "-C", path, "rev-parse", "--git-dir")
		out2, err := cmd2.Output()
		if err != nil {
			return ""
		}
		gitDir := strings.TrimSpace(string(out2))
		if !filepath.IsAbs(gitDir) {
			gitDir = filepath.Join(path, gitDir)
		}
		commonDir = filepath.Join(gitDir, commonDir)
	}

	commonDir = filepath.Clean(commonDir)

	// The main worktree is the parent of the .git directory
	if filepath.Base(commonDir) == ".git" {
		return filepath.Dir(commonDir)
	}

	return ""
}

// GitCommonDir returns the shared .git directory for the repository at
// projectPath (e.g. "/repo/.git"). Returns "" for non-git directories.
// Works for both main worktrees and linked worktrees.
// E-PENPAL-WORKTREE-WATCH: resolves the git common dir for fs watching.
func GitCommonDir(projectPath string) string {
	cmd := exec.Command("git", "-C", projectPath, "rev-parse", "--git-common-dir")
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	commonDir := strings.TrimSpace(string(out))
	if commonDir == "" || commonDir == "." {
		return ""
	}
	// --git-common-dir output is relative to the -C directory
	if !filepath.IsAbs(commonDir) {
		commonDir = filepath.Join(projectPath, commonDir)
	}
	return filepath.Clean(commonDir)
}

// GitWorktreesDir returns the path to the .git/worktrees/ directory for the
// repository that projectPath belongs to, or "" if it doesn't exist.
// E-PENPAL-WORKTREE-WATCH: resolves the worktrees metadata directory for fs watching.
func GitWorktreesDir(projectPath string) string {
	commonDir := GitCommonDir(projectPath)
	if commonDir == "" {
		return ""
	}
	wtDir := filepath.Join(commonDir, "worktrees")
	if info, err := os.Stat(wtDir); err == nil && info.IsDir() {
		return wtDir
	}
	return ""
}
