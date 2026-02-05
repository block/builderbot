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
		project.Summary = generateSummary(projectPath, thoughtsPath)
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

func generateSummary(projectPath, thoughtsPath string) string {
	// Try to get description from README first
	readmePaths := []string{
		filepath.Join(projectPath, "README.md"),
		filepath.Join(projectPath, "readme.md"),
		filepath.Join(projectPath, "README"),
	}

	for _, readmePath := range readmePaths {
		if desc := extractReadmeDescription(readmePath); desc != "" {
			return truncateSummary(desc, 140)
		}
	}

	// Fall back to generating from project name and recent file topics
	name := filepath.Base(projectPath)
	topics := extractRecentTopics(thoughtsPath)

	if len(topics) > 0 {
		return truncateSummary(humanizeName(name)+": "+strings.Join(topics, ", "), 140)
	}

	return humanizeName(name)
}

func extractReadmeDescription(path string) string {
	file, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer file.Close()

	scanner := bufio.NewScanner(file)
	var foundHeader bool
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		// Skip the title header
		if strings.HasPrefix(line, "#") && !foundHeader {
			foundHeader = true
			continue
		}
		// Skip badges and links at the start
		if strings.HasPrefix(line, "[") || strings.HasPrefix(line, "!") {
			continue
		}
		// Return first real paragraph line
		if foundHeader && line != "" {
			return line
		}
	}
	return ""
}

func extractRecentTopics(thoughtsPath string) []string {
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

	// Sort by mod time, newest first
	sort.Slice(files, func(i, j int) bool {
		return files[i].modTime.After(files[j].modTime)
	})

	// Extract topics from the 3 most recent files
	topics := make(map[string]bool)
	datePrefix := regexp.MustCompile(`^\d{4}-\d{2}-\d{2}-?`)

	for i, f := range files {
		if i >= 3 {
			break
		}
		name := strings.TrimSuffix(filepath.Base(f.path), ".md")
		// Remove date prefix
		name = datePrefix.ReplaceAllString(name, "")
		// Convert kebab-case to words
		name = strings.ReplaceAll(name, "-", " ")
		name = strings.TrimSpace(name)
		if name != "" && len(name) > 3 {
			topics[name] = true
		}
	}

	var result []string
	for topic := range topics {
		result = append(result, topic)
	}
	return result
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
