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
	Project     string
	Workspace   string // workspace display name (empty for standalone)
	ProjectPath string // absolute filesystem path to project root
	Source      string // source name (e.g., "thoughts", "docs")
	SourceType  string // source type: "thoughts", "tree", or "files"
	SourceAuto  bool   // true if source was auto-detected
	Path        string // relative to source root
	FullPath    string // relative to project root (e.g., "thoughts/plans/foo.md")
	Name        string
	ModTime     time.Time
	FileType    string // "research", "plan", or "other"
}

// Cache holds all cached data for the server
type Cache struct {
	mu sync.RWMutex

	projects []discovery.Project
	// projectFiles maps project name to its file list
	projectFiles map[string][]FileInfo
}

// New creates a new cache
func New() *Cache {
	return &Cache{
		projectFiles: make(map[string][]FileInfo),
	}
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

// FindProject returns a project by its qualified name (e.g., "Development/birdseye"
// for workspace projects, or "myproject" for standalone projects).
func (c *Cache) FindProject(qualifiedName string) *discovery.Project {
	c.mu.RLock()
	defer c.mu.RUnlock()
	for i := range c.projects {
		if c.projects[i].QualifiedName() == qualifiedName {
			p := c.projects[i]
			return &p
		}
	}
	return nil
}

// FindProjectByPath returns a project whose root is a prefix of the given
// absolute path, or nil if no project matches.
func (c *Cache) FindProjectByPath(absPath string) *discovery.Project {
	c.mu.RLock()
	defer c.mu.RUnlock()
	var best *discovery.Project
	bestLen := -1
	for i := range c.projects {
		projPath := c.projects[i].Path
		if (strings.HasPrefix(absPath, projPath+"/") || absPath == projPath) && len(projPath) > bestLen {
			p := c.projects[i]
			best = &p
			bestLen = len(projPath)
		}
	}
	return best
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

// RefreshProject rescans a single project's files across all its sources.
// projectName should be the qualified name (e.g., "Development/birdseye").
func (c *Cache) RefreshProject(projectName string) {
	project := c.FindProject(projectName)
	if project == nil {
		return
	}

	files := scanProjectSources(project)
	c.SetProjectFiles(projectName, files)

	// Update project metadata
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].QualifiedName() == projectName {
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
		go func(qn string) {
			defer wg.Done()
			c.RefreshProject(qn)
		}(p.QualifiedName())
	}
	wg.Wait()
}

// EnrichProject updates a project's git info without rescanning files.
// name should be the qualified name (e.g., "Development/birdseye").
func (c *Cache) EnrichProject(name string, git *discovery.GitInfo) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].QualifiedName() == name {
			c.projects[i].Git = git
			break
		}
	}
}

// RemoveProject removes a project from the cache.
// name should be the qualified name (e.g., "Development/birdseye").
func (c *Cache) RemoveProject(name string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].QualifiedName() == name {
			c.projects = append(c.projects[:i], c.projects[i+1:]...)
			break
		}
	}
	delete(c.projectFiles, name)
}

// RefreshProjectGitInfo re-fetches git info (branch, dirty, unstaged mod times,
// unpushed commit times) for a single project without rescanning files.
// name should be the qualified name (e.g., "Development/birdseye").
func (c *Cache) RefreshProjectGitInfo(name string) {
	project := c.FindProject(name)
	if project == nil || project.Name == "(root)" {
		return
	}
	git := discovery.GetGitInfo(project.Path)
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range c.projects {
		if c.projects[i].QualifiedName() == name {
			c.projects[i].Git = git
			break
		}
	}
}

// RescanWith replaces the project list with the given projects,
// preserving existing git info for known projects.
func (c *Cache) RescanWith(projects []discovery.Project) {
	// Preserve enrichment data (git info) for projects we already know about
	c.mu.RLock()
	existing := make(map[string]discovery.Project)
	for _, p := range c.projects {
		existing[p.QualifiedName()] = p
	}
	c.mu.RUnlock()

	for i := range projects {
		if prev, ok := existing[projects[i].QualifiedName()]; ok {
			projects[i].Git = prev.Git
		}
	}

	c.SetProjects(projects)
	c.RefreshAllProjects()
}

// scanProjectSources scans all sources of a project for markdown files
func scanProjectSources(project *discovery.Project) []FileInfo {
	var files []FileInfo

	for _, source := range project.Sources {
		if source.Type == "thoughts" || source.Type == "tree" {
			rootPath := source.RootPath
			if rootPath == "" {
				continue
			}
			filepath.Walk(rootPath, func(path string, info os.FileInfo, err error) error {
				if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
					return nil
				}
				relToSource, _ := filepath.Rel(rootPath, path)
				relToProject, _ := filepath.Rel(project.Path, path)

				fileType := "other"
				st := discovery.GetSourceType(source.Name)
				if st != nil && st.ClassifyFile != nil {
					fileType = st.ClassifyFile(relToSource)
				} else {
					if strings.Contains(relToSource, "research") {
						fileType = "research"
					} else if strings.Contains(relToSource, "plan") {
						fileType = "plan"
					}
				}

				files = append(files, FileInfo{
					Project:     project.QualifiedName(),
					Workspace:   project.WorkspaceName,
					ProjectPath: project.Path,
					Source:      source.Name,
					SourceType:  source.Type,
					SourceAuto:  source.Auto,
					Path:        relToSource,
					FullPath:    relToProject,
					Name:        filepath.Base(path),
					ModTime:     info.ModTime(),
					FileType:    fileType,
				})
				return nil
			})
		} else if source.Type == "files" {
			for _, filePath := range source.Files {
				info, err := os.Stat(filePath)
				if err != nil || info.IsDir() || !strings.HasSuffix(filePath, ".md") {
					continue
				}
				relToProject, _ := filepath.Rel(project.Path, filePath)

				fileType := "other"
				lower := strings.ToLower(filepath.Base(filePath))
				if strings.Contains(lower, "research") {
					fileType = "research"
				} else if strings.Contains(lower, "plan") {
					fileType = "plan"
				}

				files = append(files, FileInfo{
					Project:     project.QualifiedName(),
					Workspace:   project.WorkspaceName,
					ProjectPath: project.Path,
					Source:      source.Name,
					SourceType:  source.Type,
					SourceAuto:  source.Auto,
					Path:        filepath.Base(filePath),
					FullPath:    relToProject,
					Name:        filepath.Base(filePath),
					ModTime:     info.ModTime(),
					FileType:    fileType,
				})
			}
		}
	}

	// Sort by modification time, newest first
	sort.Slice(files, func(i, j int) bool {
		return files[i].ModTime.After(files[j].ModTime)
	})

	return files
}
