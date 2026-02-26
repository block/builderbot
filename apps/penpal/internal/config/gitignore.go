package config

import (
	"bufio"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// EnsureGlobalGitignore adds .penpal to the user's global gitignore file
// so that project-level .penpal/ directories are not committed.
func EnsureGlobalGitignore() {
	path := globalGitignorePath()
	if path == "" {
		return
	}

	if containsLine(path, ".penpal") {
		return
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		log.Printf("Warning: could not create gitignore directory: %v", err)
		return
	}

	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		log.Printf("Warning: could not open global gitignore: %v", err)
		return
	}
	defer f.Close()

	// Add a newline before in case the file doesn't end with one
	info, _ := f.Stat()
	if info != nil && info.Size() > 0 {
		f.WriteString("\n")
	}
	f.WriteString(".penpal\n")
	log.Printf("Added .penpal to global gitignore: %s", path)
}

// globalGitignorePath returns the path to the global gitignore file.
func globalGitignorePath() string {
	// Check the effective core.excludesFile (resolves includes from all config levels)
	out, err := exec.Command("git", "config", "core.excludesFile").Output()
	if err == nil {
		p := strings.TrimSpace(string(out))
		if p != "" {
			// Expand ~ if present
			if strings.HasPrefix(p, "~/") {
				if home, err := os.UserHomeDir(); err == nil {
					return filepath.Join(home, p[2:])
				}
			}
			return p
		}
	}

	// Default location
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".config", "git", "ignore")
}

// containsLine checks if a file contains a line matching the given text.
func containsLine(path, text string) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		if strings.TrimSpace(scanner.Text()) == text {
			return true
		}
	}
	return false
}
