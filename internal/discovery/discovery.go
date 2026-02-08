package discovery

import (
	"bufio"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/loganj/birdseye/internal/config"
)

// FileSource represents a set of files to display for a project.
type FileSource struct {
	Name     string   // display name (e.g., "thoughts", "docs")
	Type     string   // "thoughts", "tree", "files"
	RootPath string   // absolute path to tree root (for thoughts/tree types)
	Files    []string // absolute paths (for "files" type)
	Auto     bool     // true if auto-detected (thoughts/), false if user-added
}

type Project struct {
	Name          string
	Path          string // project root directory
	Sources       []FileSource
	Origin        string // "workspace" or "standalone"
	WorkspacePath string // which workspace discovered this (empty for standalone)
	WorkspaceName string // display name of the workspace (empty for standalone)
	Git           *GitInfo
	FileCount     int
	Summary       string
	LastModified  time.Time
}

// ThoughtsPath returns the thoughts source root if present, empty string otherwise.
func (p *Project) ThoughtsPath() string {
	for _, s := range p.Sources {
		if s.Type == "thoughts" {
			return s.RootPath
		}
	}
	return ""
}

// HasThoughts returns true if the project has a thoughts/ directory.
func (p *Project) HasThoughts() bool {
	return p.ThoughtsPath() != ""
}

// QualifiedName returns the workspace-qualified project identifier
// (e.g., "Development/birdseye"). This is the unique key used for
// cache lookups, comment storage, API calls, and SSE events.
func (p *Project) QualifiedName() string {
	if p.WorkspaceName != "" {
		return p.WorkspaceName + "/" + p.Name
	}
	return p.Name
}

// DetectSources finds auto-detectable file sources in a project directory.
// Currently only detects thoughts/ directories.
func DetectSources(projectPath string) []FileSource {
	var sources []FileSource
	thoughtsPath := filepath.Join(projectPath, "thoughts")
	if info, err := os.Stat(thoughtsPath); err == nil && info.IsDir() {
		sources = append(sources, FileSource{
			Name:     "thoughts",
			Type:     "thoughts",
			RootPath: thoughtsPath,
			Auto:     true,
		})
	}
	return sources
}

// DiscoverWorkspace scans a workspace directory for projects.
// ALL immediate subdirectories are treated as projects, regardless of whether
// they have thoughts/ or other known files.
func DiscoverWorkspace(workspacePath, workspaceName string) ([]Project, error) {
	entries, err := os.ReadDir(workspacePath)
	if err != nil {
		return nil, err
	}

	var projects []Project
	for _, entry := range entries {
		if !entry.IsDir() || entry.Name()[0] == '.' {
			continue
		}

		projectPath := filepath.Join(workspacePath, entry.Name())
		project := Project{
			Name:          entry.Name(),
			Path:          projectPath,
			Sources:       DetectSources(projectPath),
			Origin:        "workspace",
			WorkspacePath: workspacePath,
			WorkspaceName: workspaceName,
		}
		projects = append(projects, project)
	}

	// Also check for thoughts/ directly in workspace root
	rootThoughts := filepath.Join(workspacePath, "thoughts")
	if info, err := os.Stat(rootThoughts); err == nil && info.IsDir() {
		projects = append(projects, Project{
			Name:          "(root)",
			Path:          workspacePath,
			Sources:       []FileSource{{Name: "thoughts", Type: "thoughts", RootPath: rootThoughts, Auto: true}},
			Origin:        "workspace",
			WorkspacePath: workspacePath,
			WorkspaceName: workspaceName,
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

// LoadStandaloneProject creates a Project from an explicit path with optional configured sources.
func LoadStandaloneProject(projectPath string, cfg config.ProjectConfig) (Project, error) {
	absPath, err := filepath.Abs(projectPath)
	if err != nil {
		return Project{}, err
	}

	name := cfg.Name
	if name == "" {
		name = filepath.Base(absPath)
	}

	project := Project{
		Name:   name,
		Path:   absPath,
		Origin: "standalone",
	}

	// Auto-detect sources
	project.Sources = DetectSources(absPath)

	// Add user-configured sources
	for _, src := range cfg.Sources {
		fs := FileSource{
			Name: src.Name,
			Type: src.Type,
			Auto: false,
		}
		if src.Type == "tree" {
			fs.RootPath = filepath.Join(absPath, src.Path)
			if fs.Name == "" {
				fs.Name = src.Path
			}
		} else if src.Type == "files" {
			fs.Files = make([]string, len(src.Files))
			for i, f := range src.Files {
				fs.Files[i] = filepath.Join(absPath, f)
			}
			if fs.Name == "" {
				fs.Name = "files"
			}
		}
		project.Sources = append(project.Sources, fs)
	}

	return project, nil
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

	filePaths := make([]string, len(files))
	for i, f := range files {
		filePaths[i] = f.path
	}
	return GenerateSummaryFromFiles(filePaths)
}

// GenerateSummaryFromFiles generates a summary from pre-sorted file paths.
// Use this when you already have the recent files list (e.g. from cache)
// to avoid re-walking the directory.
func GenerateSummaryFromFiles(filePaths []string) string {
	if len(filePaths) == 0 {
		return ""
	}

	// Try to extract meaningful content from recent files
	var topics []string
	datePrefix := regexp.MustCompile(`^\d{4}-\d{2}-\d{2}-?`)

	for _, path := range filePaths {
		// Try to get the h1 title from the file
		if title := extractFileTitle(path); title != "" {
			topics = append(topics, title)
			continue
		}

		// Fall back to humanized filename
		name := strings.TrimSuffix(filepath.Base(path), ".md")
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
