package claudepath

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// candidatePaths returns common locations where the claude binary may be installed,
// in priority order. Paths use $HOME expansion since GUI apps don't get shell expansion.
func candidatePaths() []string {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil
	}

	paths := []string{
		filepath.Join(home, ".local", "bin", "claude"), // native installer (most common)
		"/opt/homebrew/bin/claude",                     // Homebrew on Apple Silicon
		"/usr/local/bin/claude",                        // Homebrew on Intel / legacy npm
	}

	// nvm: check all installed node versions
	nvmDir := filepath.Join(home, ".nvm", "versions", "node")
	if entries, err := os.ReadDir(nvmDir); err == nil {
		for _, e := range entries {
			if e.IsDir() {
				paths = append(paths, filepath.Join(nvmDir, e.Name(), "bin", "claude"))
			}
		}
	}

	return paths
}

// Resolve finds the claude binary. It checks in order:
//  1. The provided remembered path (from config) — if it still exists and is executable
//  2. exec.LookPath (works when launched from terminal with full PATH)
//  3. Well-known candidate paths (works when launched as GUI app with minimal PATH)
//
// Returns the absolute path to the claude binary, or empty string if not found.
func Resolve(remembered string) string {
	// Check remembered path first
	if remembered != "" && IsExecutable(remembered) {
		return remembered
	}

	// Try PATH lookup (works when launched from terminal)
	if p, err := exec.LookPath("claude"); err == nil {
		return p
	}

	// Probe well-known locations (works when launched as GUI app)
	for _, p := range candidatePaths() {
		if IsExecutable(p) {
			return p
		}
	}

	return ""
}

// IsExecutable returns true if path exists, is a file, and has an execute bit set.
func IsExecutable(path string) bool {
	info, err := os.Stat(path)
	if err != nil {
		return false
	}
	// Follow symlinks (Stat already does), check it's a file with some execute bit
	return !info.IsDir() && info.Mode().Perm()&0111 != 0
}

// Version runs `claude --version` and returns the trimmed output, or empty string on error.
func Version(claudePath string) string {
	if claudePath == "" {
		return ""
	}
	out, err := exec.Command(claudePath, "--version").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}
