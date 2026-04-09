// Package gitignore provides a pure-Go gitignore matcher that replaces
// subprocess calls to `git check-ignore`. It parses .gitignore files,
// .git/info/exclude, and the global gitignore, then answers directory-ignore
// queries in-process.
// E-PENPAL-SCAN: pure-Go gitignore matching — zero subprocess overhead.
package gitignore

import (
	"bufio"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
)

// Matcher evaluates whether directories are gitignored within a repository.
// It loads and caches .gitignore files, .git/info/exclude, and the global
// gitignore, then answers IsIgnoredDir queries without spawning subprocesses.
type Matcher struct {
	repoRoot string // absolute path to repo root (contains .git dir)
	gitDir   string // absolute path to the git directory

	mu        sync.RWMutex
	global    []rule            // global gitignore rules
	exclude   []rule            // .git/info/exclude rules
	fileCache map[string][]rule // abs dir path -> parsed .gitignore rules (nil = no file)
	dirCache  map[string]bool   // abs dir path -> ignored?
}

// New creates a Matcher for the given project path. It walks upward to find
// the git repository root. Returns nil if the path is not inside a git repo.
func New(projectPath string) *Matcher {
	repoRoot, gitDir := findRepo(projectPath)
	if repoRoot == "" {
		return nil
	}

	m := &Matcher{
		repoRoot:  repoRoot,
		gitDir:    gitDir,
		fileCache: make(map[string][]rule),
		dirCache:  make(map[string]bool),
	}

	// Load global gitignore.
	if globalPath := globalGitignorePath(); globalPath != "" {
		m.global = parseFile(globalPath, m.repoRoot)
	}

	// Load .git/info/exclude.
	excludePath := filepath.Join(gitDir, "info", "exclude")
	m.exclude = parseFile(excludePath, m.repoRoot)

	return m
}

// IsIgnoredDir returns true if the given absolute directory path is gitignored.
// Results are cached for the lifetime of the Matcher.
func (m *Matcher) IsIgnoredDir(absDir string) bool {
	if m == nil {
		return false
	}

	m.mu.RLock()
	if result, ok := m.dirCache[absDir]; ok {
		m.mu.RUnlock()
		return result
	}
	m.mu.RUnlock()

	result := m.computeIgnored(absDir)

	m.mu.Lock()
	m.dirCache[absDir] = result
	m.mu.Unlock()

	return result
}

// computeIgnored evaluates whether absDir is ignored by walking up the
// directory tree and applying gitignore rules in precedence order.
func (m *Matcher) computeIgnored(absDir string) bool {
	// The repo root itself is never ignored.
	if absDir == m.repoRoot {
		return false
	}

	// If absDir is outside the repo, not ignored.
	rel, err := filepath.Rel(m.repoRoot, absDir)
	if err != nil || strings.HasPrefix(rel, "..") {
		return false
	}

	// Check parent first — if parent is ignored, child is too
	// (negation cannot re-include under an ignored parent).
	parent := filepath.Dir(absDir)
	if parent != m.repoRoot && m.IsIgnoredDir(parent) {
		return true
	}

	// Build the path relative to repo root using forward slashes.
	relSlash := filepath.ToSlash(rel)

	// Evaluate rules in precedence order (last match wins within each level,
	// higher-precedence levels override lower). Precedence order:
	// 1. global gitignore (lowest)
	// 2. .git/info/exclude
	// 3. .gitignore files from repo root down to parent of absDir (highest)
	//
	// We scan all rule sources and track the last match.
	matched := false
	ignored := false

	// Global rules.
	if hit, neg := matchRules(m.global, relSlash, true); hit {
		matched = true
		ignored = !neg
	}

	// .git/info/exclude rules.
	if hit, neg := matchRules(m.exclude, relSlash, true); hit {
		matched = true
		ignored = !neg
	}

	// Per-directory .gitignore files, from repo root down to parent(absDir).
	dirs := ancestorDirs(m.repoRoot, absDir)
	for _, dir := range dirs {
		rules := m.loadGitignore(dir)
		if len(rules) == 0 {
			continue
		}
		// For .gitignore in a subdirectory, paths are relative to that dir.
		subRel, _ := filepath.Rel(dir, absDir)
		subRelSlash := filepath.ToSlash(subRel)
		if hit, neg := matchRules(rules, subRelSlash, true); hit {
			matched = true
			ignored = !neg
		}
	}

	if !matched {
		return false
	}
	return ignored
}

// loadGitignore returns parsed rules for the .gitignore in dir, using cache.
func (m *Matcher) loadGitignore(dir string) []rule {
	m.mu.RLock()
	if rules, ok := m.fileCache[dir]; ok {
		m.mu.RUnlock()
		return rules
	}
	m.mu.RUnlock()

	path := filepath.Join(dir, ".gitignore")
	rules := parseFile(path, dir)

	m.mu.Lock()
	m.fileCache[dir] = rules
	m.mu.Unlock()

	return rules
}

// ancestorDirs returns directories from repoRoot down to (and including)
// the parent of target. The list is ordered root-first.
func ancestorDirs(repoRoot, target string) []string {
	var dirs []string
	dir := filepath.Dir(target) // parent of target
	for {
		dirs = append(dirs, dir)
		if dir == repoRoot {
			break
		}
		next := filepath.Dir(dir)
		if next == dir {
			break
		}
		dir = next
	}
	// Reverse to get root-first order.
	for i, j := 0, len(dirs)-1; i < j; i, j = i+1, j-1 {
		dirs[i], dirs[j] = dirs[j], dirs[i]
	}
	return dirs
}

// findRepo walks upward from path to find the git repository root.
// Returns (repoRoot, gitDir) or ("", "") if not in a git repo.
func findRepo(path string) (string, string) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", ""
	}
	dir := abs
	for {
		gitEntry := filepath.Join(dir, ".git")
		fi, err := os.Lstat(gitEntry)
		if err == nil {
			if fi.IsDir() {
				return dir, gitEntry
			}
			// .git file (worktree/submodule) — parse gitdir line.
			if gitDir := parseGitFile(gitEntry, dir); gitDir != "" {
				return dir, gitDir
			}
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", ""
		}
		dir = parent
	}
}

// parseGitFile reads a .git file (worktree/submodule format) and returns
// the resolved gitdir path.
func parseGitFile(path, baseDir string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	line := strings.TrimSpace(string(data))
	if !strings.HasPrefix(line, "gitdir: ") {
		return ""
	}
	gitDir := strings.TrimPrefix(line, "gitdir: ")
	if !filepath.IsAbs(gitDir) {
		gitDir = filepath.Join(baseDir, gitDir)
	}
	gitDir = filepath.Clean(gitDir)
	if fi, err := os.Stat(gitDir); err == nil && fi.IsDir() {
		return gitDir
	}
	return ""
}

var (
	globalIgnoreOnce sync.Once
	globalIgnorePath string
)

// globalGitignorePath returns the path to the global gitignore file.
// E-PENPAL-SCAN: cached via sync.Once to avoid repeated subprocess calls.
func globalGitignorePath() string {
	globalIgnoreOnce.Do(func() {
		// Try git config first
		out, err := exec.Command("git", "config", "core.excludesFile").Output()
		if err == nil {
			p := strings.TrimSpace(string(out))
			if p != "" {
				if strings.HasPrefix(p, "~/") {
					if home, err := os.UserHomeDir(); err == nil {
						globalIgnorePath = filepath.Join(home, p[2:])
						return
					}
				}
				globalIgnorePath = p
				return
			}
		}
		// Default location
		home, err := os.UserHomeDir()
		if err != nil {
			return
		}
		globalIgnorePath = filepath.Join(home, ".config", "git", "ignore")
	})
	return globalIgnorePath
}

// --- Pattern parsing ---

type rule struct {
	negated  bool
	dirOnly  bool
	anchored bool // pattern contains "/" or starts with "/"
	pattern  string
}

// parseFile reads and parses a .gitignore file. Returns nil if the file
// doesn't exist. baseDir is the directory containing the ignore file
// (used for anchored pattern matching).
func parseFile(path, baseDir string) []rule {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()

	var rules []rule
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := scanner.Text()
		r, ok := parseLine(line)
		if ok {
			rules = append(rules, r)
		}
	}
	return rules
}

// trimTrailingWhitespace trims trailing spaces/tabs, but preserves a space
// escaped with a preceding backslash (e.g., "foo\ " keeps the trailing space).
func trimTrailingWhitespace(s string) string {
	end := len(s)
	for end > 0 && (s[end-1] == ' ' || s[end-1] == '\t') {
		end--
	}
	// If the last non-whitespace char is a backslash and there was whitespace
	// after it, keep exactly one space (the escaped one).
	if end > 0 && end < len(s) && s[end-1] == '\\' {
		return s[:end-1] + " "
	}
	return s[:end]
}

// parseLine parses a single .gitignore line into a rule.
func parseLine(line string) (rule, bool) {
	// Strip trailing whitespace (unless escaped with backslash).
	// E-PENPAL-SCAN: preserve backslash-escaped trailing space per gitignore spec.
	line = trimTrailingWhitespace(line)

	// Skip empty lines and comments.
	if line == "" || line[0] == '#' {
		return rule{}, false
	}

	var r rule

	// Handle negation.
	if line[0] == '!' {
		r.negated = true
		line = line[1:]
		if line == "" {
			return rule{}, false
		}
	}

	// Handle leading backslash escape (e.g., \# or \!).
	if line[0] == '\\' && len(line) > 1 && (line[1] == '#' || line[1] == '!' || line[1] == ' ') {
		line = line[1:]
	}

	// Trailing "/" means directory-only.
	if strings.HasSuffix(line, "/") {
		r.dirOnly = true
		line = strings.TrimRight(line, "/")
		if line == "" {
			return rule{}, false
		}
	}

	// Leading "/" means anchored to the base directory.
	if line[0] == '/' {
		r.anchored = true
		line = line[1:]
		if line == "" {
			return rule{}, false
		}
	}

	// If the pattern contains a "/" (after stripping leading/trailing),
	// it's anchored.
	if strings.Contains(line, "/") {
		r.anchored = true
	}

	r.pattern = line
	return r, true
}

// --- Pattern matching ---

// matchRules evaluates rules against a slash-separated relative path.
// Returns (matched, negated). Last matching rule wins.
func matchRules(rules []rule, relPath string, isDir bool) (bool, bool) {
	matched := false
	negated := false

	for _, r := range rules {
		// dirOnly patterns only match directories.
		if r.dirOnly && !isDir {
			continue
		}

		if r.anchored {
			// Anchored: match against the full relative path.
			if globMatch(r.pattern, relPath) {
				matched = true
				negated = r.negated
			}
		} else {
			// Unanchored: match against basename, or against each
			// suffix of the path.
			if !strings.Contains(r.pattern, "/") {
				// Simple basename match.
				base := relPath
				if idx := strings.LastIndex(relPath, "/"); idx >= 0 {
					base = relPath[idx+1:]
				}
				if globMatch(r.pattern, base) {
					matched = true
					negated = r.negated
				}
			} else {
				// Pattern with "/" but not anchored — match against
				// path suffixes.
				if globMatch(r.pattern, relPath) {
					matched = true
					negated = r.negated
				}
				// Also try matching against each path suffix.
				for i := 0; i < len(relPath); i++ {
					if relPath[i] == '/' {
						suffix := relPath[i+1:]
						if globMatch(r.pattern, suffix) {
							matched = true
							negated = r.negated
						}
					}
				}
			}
		}
	}

	return matched, negated
}

// globMatch matches a gitignore-style glob pattern against a string.
// Supports *, **, ?, and [abc] character classes.
// Paths use forward slashes.
func globMatch(pattern, name string) bool {
	return doMatch(pattern, name)
}

func doMatch(pattern, name string) bool {
	for len(pattern) > 0 {
		switch pattern[0] {
		case '*':
			if len(pattern) > 1 && pattern[1] == '*' {
				// "**" — match zero or more path segments.
				pattern = pattern[2:]

				// "**/" at start or middle — skip the slash.
				if len(pattern) > 0 && pattern[0] == '/' {
					pattern = pattern[1:]
				}

				// If pattern is exhausted, match everything.
				if len(pattern) == 0 {
					return true
				}

				// Try matching the remainder against every suffix.
				for i := 0; i <= len(name); i++ {
					if doMatch(pattern, name[i:]) {
						return true
					}
					// Only try positions at start or after '/'.
					for i < len(name) && name[i] != '/' {
						i++
					}
				}
				return false
			}

			// Single "*" — match any characters except "/".
			pattern = pattern[1:]
			if len(pattern) == 0 {
				// "*" at end — match if no "/" remains.
				return !strings.Contains(name, "/")
			}
			for i := 0; i <= len(name); i++ {
				if doMatch(pattern, name[i:]) {
					return true
				}
				if i < len(name) && name[i] == '/' {
					break
				}
			}
			return false

		case '?':
			if len(name) == 0 || name[0] == '/' {
				return false
			}
			pattern = pattern[1:]
			name = name[1:]

		case '[':
			if len(name) == 0 || name[0] == '/' {
				return false
			}
			// Parse character class.
			end := strings.IndexByte(pattern, ']')
			if end < 0 {
				return false // malformed
			}
			class := pattern[1:end]
			ch := name[0]
			matched := matchCharClass(class, ch)
			if !matched {
				return false
			}
			pattern = pattern[end+1:]
			name = name[1:]

		case '\\':
			// Escape next character.
			if len(pattern) > 1 {
				pattern = pattern[1:]
				if len(name) == 0 || name[0] != pattern[0] {
					return false
				}
				pattern = pattern[1:]
				name = name[1:]
			} else {
				return false
			}

		default:
			if len(name) == 0 || pattern[0] != name[0] {
				return false
			}
			pattern = pattern[1:]
			name = name[1:]
		}
	}

	return len(name) == 0
}

// matchCharClass checks if ch matches a [...] character class.
func matchCharClass(class string, ch byte) bool {
	negated := false
	if len(class) > 0 && class[0] == '!' {
		negated = true
		class = class[1:]
	}

	matched := false
	i := 0
	for i < len(class) {
		if i+2 < len(class) && class[i+1] == '-' {
			// Range: a-z.
			if ch >= class[i] && ch <= class[i+2] {
				matched = true
			}
			i += 3
		} else {
			if ch == class[i] {
				matched = true
			}
			i++
		}
	}

	if negated {
		return !matched
	}
	return matched
}
