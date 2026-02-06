package cache

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/loganj/birdseye/internal/discovery"
)

// FileInfo represents a cached file
type FileInfo struct {
	Project  string
	Path     string // relative to thoughts dir
	Name     string
	ModTime  time.Time
	FileType string // "research", "plan", or "other"
}

// Cache holds all cached data for the server
type Cache struct {
	mu sync.RWMutex

	root     string
	projects []discovery.Project
	// projectFiles maps project name to its file list
	projectFiles map[string][]FileInfo
}

// New creates a new cache
func New(root string) *Cache {
	return &Cache{
		root:         root,
		projectFiles: make(map[string][]FileInfo),
	}
}

// Root returns the root directory
func (c *Cache) Root() string {
	return c.root
}

// SetProjects updates the projects list
func (c *Cache) SetProjects(projects []discovery.Project) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.projects = projects
}

// Projects returns a copy of the projects list
func (c *Cache) Projects() []discovery.Project {
	c.mu.RLock()
	defer c.mu.RUnlock()
	result := make([]discovery.Project, len(c.projects))
	copy(result, c.projects)
	return result
}

// ProjectsSortedByModTime returns projects sorted by last modified time
func (c *Cache) ProjectsSortedByModTime() []discovery.Project {
	projects := c.Projects()
	sort.Slice(projects, func(i, j int) bool {
		return projects[i].LastModified.After(projects[j].LastModified)
	})
	return projects
}

// FindProject returns a project by name
func (c *Cache) FindProject(name string) *discovery.Project {
	c.mu.RLock()
	defer c.mu.RUnlock()
	for i := range c.projects {
		if c.projects[i].Name == name {
			p := c.projects[i]
			return &p
		}
	}
	return nil
}

// SetProjectFiles updates the file list for a project
func (c *Cache) SetProjectFiles(projectName string, files []FileInfo) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.projectFiles[projectName] = files
}

// ProjectFiles returns the cached file list for a project
func (c *Cache) ProjectFiles(projectName string) []FileInfo {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if files, ok := c.projectFiles[projectName]; ok {
		result := make([]FileInfo, len(files))
		copy(result, files)
		return result
	}
	return nil
}

// AllFiles returns all files across all projects, sorted by modification time
func (c *Cache) AllFiles(limit int) []FileInfo {
	c.mu.RLock()
	defer c.mu.RUnlock()

	var all []FileInfo
	for _, files := range c.projectFiles {
		all = append(all, files...)
	}

	sort.Slice(all, func(i, j int) bool {
		return all[i].ModTime.After(all[j].ModTime)
	})

	if limit > 0 && len(all) > limit {
		all = all[:limit]
	}
	return all
}

// RefreshProject rescans a single project's files
func (c *Cache) RefreshProject(projectName string) {
	project := c.FindProject(projectName)
	if project == nil {
		return
	}

	files := scanProjectFiles(project.ThoughtsPath, projectName)
	c.SetProjectFiles(projectName, files)

	// Update project metadata
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].Name == projectName {
			c.projects[i].FileCount = len(files)
			if len(files) > 0 {
				c.projects[i].LastModified = files[0].ModTime
			}
			break
		}
	}
}

// RefreshAllProjects rescans all projects' files and updates metadata
func (c *Cache) RefreshAllProjects() {
	projects := c.Projects()
	var wg sync.WaitGroup
	for _, p := range projects {
		wg.Add(1)
		go func(name string) {
			defer wg.Done()
			c.RefreshProject(name)
		}(p.Name)
	}
	wg.Wait()
}

// EnrichProject updates a project's git info and summary without rescanning files
func (c *Cache) EnrichProject(name string, git *discovery.GitInfo, summary string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].Name == name {
			c.projects[i].Git = git
			c.projects[i].Summary = summary
			break
		}
	}
}

// RemoveProject removes a project from the cache
func (c *Cache) RemoveProject(name string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].Name == name {
			c.projects = append(c.projects[:i], c.projects[i+1:]...)
			break
		}
	}
	delete(c.projectFiles, name)
}

// RefreshProjectGitInfo re-fetches git info (branch, dirty, unstaged mod times,
// unpushed commit times) for a single project without rescanning files.
func (c *Cache) RefreshProjectGitInfo(name string) {
	project := c.FindProject(name)
	if project == nil || project.Name == "(root)" {
		return
	}
	git := discovery.GetGitInfo(project.Path)
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].Name == name {
			c.projects[i].Git = git
			break
		}
	}
}

// RescanProjects rescans the root directory for projects using the fast path,
// preserving existing git info and summaries for known projects.
func (c *Cache) RescanProjects() error {
	projects, err := discovery.FindProjectsFast(c.root)
	if err != nil {
		return err
	}

	// Preserve enrichment data (git info, summary) for projects we already know about
	c.mu.RLock()
	existing := make(map[string]discovery.Project)
	for _, p := range c.projects {
		existing[p.Name] = p
	}
	c.mu.RUnlock()

	for i := range projects {
		if prev, ok := existing[projects[i].Name]; ok {
			projects[i].Git = prev.Git
			projects[i].Summary = prev.Summary
		}
	}

	c.SetProjects(projects)
	c.RefreshAllProjects()
	return nil
}

// scanProjectFiles scans a project's thoughts directory for files
func scanProjectFiles(thoughtsPath, projectName string) []FileInfo {
	var files []FileInfo

	filepath.Walk(thoughtsPath, func(path string, info os.FileInfo, err error) error {
		if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
			return nil
		}
		relPath, _ := filepath.Rel(thoughtsPath, path)

		fileType := "other"
		if strings.Contains(relPath, "research") {
			fileType = "research"
		} else if strings.Contains(relPath, "plan") {
			fileType = "plan"
		}

		files = append(files, FileInfo{
			Project:  projectName,
			Path:     relPath,
			Name:     filepath.Base(path),
			ModTime:  info.ModTime(),
			FileType: fileType,
		})
		return nil
	})

	// Sort by modification time, newest first
	sort.Slice(files, func(i, j int) bool {
		return files[i].ModTime.After(files[j].ModTime)
	})

	return files
}
