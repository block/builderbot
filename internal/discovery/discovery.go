package discovery

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type Project struct {
	Name         string
	Path         string
	ThoughtsPath string
	Git          *GitInfo
	FileCount    int
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
