package discovery

import (
	"bufio"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
)

type Project struct {
	Name         string
	Path         string
	ThoughtsPath string
	Git          *GitInfo
	FileCount    int
	Summary      string
	LastModified time.Time
}

func FindProjects(root string) ([]Project, error) {
	var projects []Project

	entries, err := os.ReadDir(root)
	if err != nil {
		return nil, err
	}

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		if entry.Name()[0] == '.' {
			continue
		}

		projectPath := filepath.Join(root, entry.Name())
		thoughtsPath := filepath.Join(projectPath, "thoughts")

		info, err := os.Stat(thoughtsPath)
		if err != nil || !info.IsDir() {
			continue
		}

		project := Project{
			Name:         entry.Name(),
			Path:         projectPath,
			ThoughtsPath: thoughtsPath,
		}
		project.Git = GetGitInfo(projectPath)
		project.FileCount = countMdFiles(thoughtsPath)
		project.Summary = GenerateSummary(projectPath, thoughtsPath)
		project.LastModified = getLastModified(thoughtsPath)

		projects = append(projects, project)
	}

	// Also check for thoughts/ directly in root (~/Development/thoughts)
	rootThoughts := filepath.Join(root, "thoughts")
	if info, err := os.Stat(rootThoughts); err == nil && info.IsDir() {
		project := Project{
			Name:         "(root)",
			Path:         root,
			ThoughtsPath: rootThoughts,
		}
		project.Git = nil // root isn't a git repo
		project.FileCount = countMdFiles(rootThoughts)
		project.Summary = "Cross-project notes and research"
		project.LastModified = getLastModified(rootThoughts)
		projects = append(projects, project)
	}

	sort.Slice(projects, func(i, j int) bool {
		// Put (root) first, then sort alphabetically
		if projects[i].Name == "(root)" {
			return true
		}
		if projects[j].Name == "(root)" {
			return false
		}
		return strings.ToLower(projects[i].Name) < strings.ToLower(projects[j].Name)
	})

	return projects, nil
}

// FindProjectsFast discovers projects by only checking directory structure.
// No git commands, file counting, or summary generation - just checks which
// projects have a thoughts/ subdirectory. Use cache.RefreshAllProjects() after
// to populate file lists, counts, and mod times.
func FindProjectsFast(root string) ([]Project, error) {
	var projects []Project

	entries, err := os.ReadDir(root)
	if err != nil {
		return nil, err
	}

	for _, entry := range entries {
		if !entry.IsDir() || entry.Name()[0] == '.' {
			continue
		}

		projectPath := filepath.Join(root, entry.Name())
		thoughtsPath := filepath.Join(projectPath, "thoughts")

		info, err := os.Stat(thoughtsPath)
		if err != nil || !info.IsDir() {
			continue
		}

		projects = append(projects, Project{
			Name:         entry.Name(),
			Path:         projectPath,
			ThoughtsPath: thoughtsPath,
		})
	}

	// Also check for thoughts/ directly in root
	rootThoughts := filepath.Join(root, "thoughts")
	if info, err := os.Stat(rootThoughts); err == nil && info.IsDir() {
		projects = append(projects, Project{
			Name:         "(root)",
			Path:         root,
			ThoughtsPath: rootThoughts,
		})
	}

	sort.Slice(projects, func(i, j int) bool {
		if projects[i].Name == "(root)" {
			return true
		}
		if projects[j].Name == "(root)" {
			return false
		}
		return strings.ToLower(projects[i].Name) < strings.ToLower(projects[j].Name)
	})

	return projects, nil
}

func countMdFiles(dir string) int {
	count := 0
	filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() && strings.HasSuffix(path, ".md") {
			count++
		}
		return nil
	})
	return count
}

func getLastModified(thoughtsPath string) time.Time {
	var latest time.Time
	filepath.Walk(thoughtsPath, func(path string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() && strings.HasSuffix(path, ".md") {
			if info.ModTime().After(latest) {
				latest = info.ModTime()
			}
		}
		return nil
	})
	return latest
}

// GenerateSummary generates a summary from recent thoughts files.
func GenerateSummary(projectPath, thoughtsPath string) string {
	// Generate summary from thoughts files
	files := getRecentThoughtsFiles(thoughtsPath, 5)
	if len(files) == 0 {
		return ""
	}

	// Try to extract meaningful content from recent files
	var topics []string
	datePrefix := regexp.MustCompile(`^\d{4}-\d{2}-\d{2}-?`)

	for _, f := range files {
		// Try to get the h1 title from the file
		if title := extractFileTitle(f.path); title != "" {
			topics = append(topics, title)
			continue
		}

		// Fall back to humanized filename
		name := strings.TrimSuffix(filepath.Base(f.path), ".md")
		name = datePrefix.ReplaceAllString(name, "")
		name = strings.ReplaceAll(name, "-", " ")
		name = strings.TrimSpace(name)
		if name != "" && len(name) > 3 {
			topics = append(topics, name)
		}
	}

	if len(topics) == 0 {
		return ""
	}

	// Deduplicate and limit
	seen := make(map[string]bool)
	var unique []string
	for _, t := range topics {
		lower := strings.ToLower(t)
		if !seen[lower] && len(t) > 3 {
			seen[lower] = true
			unique = append(unique, t)
			if len(unique) >= 3 {
				break
			}
		}
	}

	return truncateSummary(strings.Join(unique, "; "), 140)
}

func getRecentThoughtsFiles(thoughtsPath string, limit int) []struct {
	path    string
	modTime time.Time
} {
	var files []struct {
		path    string
		modTime time.Time
	}

	filepath.Walk(thoughtsPath, func(path string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() && strings.HasSuffix(path, ".md") {
			files = append(files, struct {
				path    string
				modTime time.Time
			}{path, info.ModTime()})
		}
		return nil
	})

	sort.Slice(files, func(i, j int) bool {
		return files[i].modTime.After(files[j].modTime)
	})

	if len(files) > limit {
		files = files[:limit]
	}
	return files
}

func extractFileTitle(path string) string {
	file, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer file.Close()

	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		// Skip YAML frontmatter
		if line == "---" {
			inFrontmatter := true
			for scanner.Scan() {
				if strings.TrimSpace(scanner.Text()) == "---" {
					inFrontmatter = false
					break
				}
			}
			if inFrontmatter {
				return ""
			}
			continue
		}
		// Look for h1
		if strings.HasPrefix(line, "# ") {
			title := strings.TrimPrefix(line, "# ")
			// Clean up common suffixes
			title = strings.TrimSuffix(title, " Implementation Plan")
			title = strings.TrimSuffix(title, " Plan")
			title = strings.TrimSuffix(title, " Research")
			return title
		}
		// If first non-empty line isn't a header, give up
		return ""
	}
	return ""
}

func humanizeName(name string) string {
	// Convert kebab-case or snake_case to Title Case
	name = strings.ReplaceAll(name, "-", " ")
	name = strings.ReplaceAll(name, "_", " ")
	words := strings.Fields(name)
	for i, word := range words {
		if len(word) > 0 {
			words[i] = strings.ToUpper(word[:1]) + word[1:]
		}
	}
	return strings.Join(words, " ")
}

func truncateSummary(s string, max int) string {
	if len(s) <= max {
		return s
	}
	// Try to cut at word boundary
	truncated := s[:max]
	if lastSpace := strings.LastIndex(truncated, " "); lastSpace > max-30 {
		truncated = truncated[:lastSpace]
	}
	return truncated + "..."
}
