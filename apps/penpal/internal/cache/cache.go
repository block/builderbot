package cache

import (
	"bufio"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/loganj/penpal/internal/discovery"
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
	Title       string // H1 heading extracted from markdown files
	ModTime     time.Time
	FileType    string // "research", "plan", or "other"
	Worktree    string // worktree name, empty for main
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

// FindProject returns a project by its qualified name (e.g., "Development/penpal"
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
	absPath = filepath.Clean(absPath)
	var best *discovery.Project
	bestLen := -1
	for i := range c.projects {
		projPath := filepath.Clean(c.projects[i].Path)
		if (strings.HasPrefix(absPath, projPath+"/") || absPath == projPath) && len(projPath) > bestLen {
			p := c.projects[i]
			best = &p
			bestLen = len(projPath)
		}
	}
	return best
}

// FindProjectByPathWithWorktree returns a project and the worktree name for a
// given absolute path. If the path is inside a worktree of a known project,
// it returns the parent project and the worktree name. If the path is inside
// the main project, worktree is empty.
func (c *Cache) FindProjectByPathWithWorktree(absPath string) (project *discovery.Project, worktree string) {
	absPath = filepath.Clean(absPath)

	// First, try direct project match (handles main worktree and non-worktree projects)
	project = c.FindProjectByPath(absPath)
	if project != nil {
		// Check if the path is inside a worktree of this project
		for _, wt := range project.Worktrees {
			if !wt.IsMain && (strings.HasPrefix(absPath, wt.Path+"/") || absPath == wt.Path) {
				return project, wt.Name
			}
		}
		return project, ""
	}

	// Path didn't match any project directly. It might be inside a worktree
	// that lives outside the project directory (e.g., at an arbitrary path).
	c.mu.RLock()
	defer c.mu.RUnlock()
	for i := range c.projects {
		for _, wt := range c.projects[i].Worktrees {
			if !wt.IsMain && (strings.HasPrefix(absPath, wt.Path+"/") || absPath == wt.Path) {
				p := c.projects[i]
				return &p, wt.Name
			}
		}
	}

	return nil, ""
}

// WorktreePath returns the filesystem path for a worktree of the given project.
// If worktreeName is empty, returns the project's main path.
func (c *Cache) WorktreePath(projectName, worktreeName string) string {
	if worktreeName == "" {
		project := c.FindProject(projectName)
		if project == nil {
			return ""
		}
		return project.Path
	}

	project := c.FindProject(projectName)
	if project == nil {
		return ""
	}

	for _, wt := range project.Worktrees {
		if wt.Name == worktreeName {
			return wt.Path
		}
	}
	return ""
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

// FindFile returns a specific file from the cache by project and file path.
// Returns nil if the file is not found.
func (c *Cache) FindFile(projectName, filePath string) *FileInfo {
	c.mu.RLock()
	defer c.mu.RUnlock()

	files, ok := c.projectFiles[projectName]
	if !ok {
		return nil
	}

	for i := range files {
		if files[i].FullPath == filePath {
			return &files[i]
		}
	}
	return nil
}

// RefreshProject rescans a single project's files across all its sources.
// projectName should be the qualified name (e.g., "Development/penpal").
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
// name should be the qualified name (e.g., "Development/penpal").
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
// name should be the qualified name (e.g., "Development/penpal").
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
// name should be the qualified name (e.g., "Development/penpal").
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

// EnrichTitles fills in missing Title fields for files in the given project.
// This is called on demand when a user views a project, so titles appear
// immediately without waiting for a full background rescan.
func (c *Cache) EnrichTitles(projectName string) {
	c.mu.Lock()
	defer c.mu.Unlock()

	files, ok := c.projectFiles[projectName]
	if !ok {
		return
	}

	changed := false
	for i := range files {
		if files[i].Title == "" {
			absPath := filepath.Join(files[i].ProjectPath, files[i].FullPath)
			if title := extractTitle(absPath); title != "" {
				files[i].Title = title
				changed = true
			}
		}
	}
	if changed {
		c.projectFiles[projectName] = files
	}
}

// extractTitle reads the first few lines of a markdown file and returns the
// text of the first H1 heading (line starting with "# "), or empty string.
func extractTitle(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for i := 0; i < 20 && scanner.Scan(); i++ {
		line := scanner.Text()
		if strings.HasPrefix(line, "# ") {
			return strings.TrimSpace(line[2:])
		}
	}
	return ""
}

// ScanProjectSourcesForWorktree scans a project's sources remapped to a worktree path.
// Each source's RootPath under the project is remapped to the equivalent path under
// the worktree. Sources whose directory doesn't exist in the worktree are skipped.
func ScanProjectSourcesForWorktree(project *discovery.Project, worktreePath string) []FileInfo {
	// Build a temporary project with remapped sources
	wtProject := *project
	wtProject.Path = worktreePath
	var remapped []discovery.FileSource
	for _, s := range project.Sources {
		ns := s
		if s.RootPath != "" {
			rel, err := filepath.Rel(project.Path, s.RootPath)
			if err != nil {
				continue
			}
			newRoot := filepath.Join(worktreePath, rel)
			if _, err := os.Stat(newRoot); err != nil {
				continue // source dir doesn't exist in worktree
			}
			ns.RootPath = newRoot
		}
		if len(s.Files) > 0 {
			// Remap absolute file paths from main project to worktree
			var remappedFiles []string
			for _, f := range s.Files {
				rel, err := filepath.Rel(project.Path, f)
				if err != nil {
					continue
				}
				newPath := filepath.Join(worktreePath, rel)
				if _, err := os.Stat(newPath); err == nil {
					remappedFiles = append(remappedFiles, newPath)
				}
			}
			if len(remappedFiles) == 0 {
				continue // no files exist in worktree
			}
			ns.Files = remappedFiles
		}
		remapped = append(remapped, ns)
	}
	wtProject.Sources = remapped
	return scanProjectSources(&wtProject)
}

// scanProjectSources scans all sources of a project for markdown files.
// Files are de-duplicated by project-relative path: if multiple sources cover
// the same file, only the first source's entry is kept. This means auto-detected
// sources (which come first) take priority over manual ones.
func scanProjectSources(project *discovery.Project) []FileInfo {
	var files []FileInfo
	seen := make(map[string]bool) // project-relative paths already claimed

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

				if seen[relToProject] {
					return nil // already claimed by an earlier source
				}

				fileType := "other"
				stName := source.SourceTypeName
				if stName == "" {
					stName = source.Name
				}
				st := discovery.GetSourceType(stName)
				if st != nil && st.ClassifyFile != nil {
					fileType = st.ClassifyFile(relToSource)
					if fileType == "" {
						return nil // skip this file
					}
				} else {
					if strings.Contains(relToSource, "research") {
						fileType = "research"
					} else if strings.Contains(relToSource, "plan") {
						fileType = "plan"
					}
				}

				title := extractTitle(path)

				seen[relToProject] = true
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
					Title:       title,
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

				if seen[relToProject] {
					continue // already claimed by an earlier source
				}

				fileType := "other"
				lower := strings.ToLower(filepath.Base(filePath))
				if strings.Contains(lower, "research") {
					fileType = "research"
				} else if strings.Contains(lower, "plan") {
					fileType = "plan"
				}

				seen[relToProject] = true
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
