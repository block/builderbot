package cache

import (
	"bufio"
	"bytes"
	"errors"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/loganj/penpal/internal/discovery"
)

// gitIgnoreChecker uses a persistent `git check-ignore --stdin` process to
// test whether paths are gitignored. A single subprocess handles all queries,
// avoiding per-directory process spawn overhead.
// E-PENPAL-SCAN: gitignore-aware directory skipping.
type gitIgnoreChecker struct {
	isGitRepo bool
	stdin     io.WriteCloser
	scanner   *bufio.Scanner
	cmd       *exec.Cmd
}

func newGitIgnoreChecker(projectPath string) *gitIgnoreChecker {
	g := &gitIgnoreChecker{}
	if exec.Command("git", "-C", projectPath, "rev-parse", "--git-dir").Run() != nil {
		return g
	}
	g.isGitRepo = true
	// Start a persistent check-ignore process. With -v -n -z, every input
	// path produces exactly 4 NUL-delimited fields: source, linenum, pattern,
	// pathname. For non-ignored paths, source is empty.
	g.cmd = exec.Command("git", "-C", projectPath, "check-ignore", "--stdin", "-z", "-v", "-n")
	stdin, err := g.cmd.StdinPipe()
	if err != nil {
		g.isGitRepo = false
		return g
	}
	stdout, err := g.cmd.StdoutPipe()
	if err != nil {
		g.isGitRepo = false
		return g
	}
	if err := g.cmd.Start(); err != nil {
		g.isGitRepo = false
		return g
	}
	g.stdin = stdin
	g.scanner = bufio.NewScanner(stdout)
	g.scanner.Split(scanNul)
	return g
}

func (g *gitIgnoreChecker) IsIgnored(path string) bool {
	if !g.isGitRepo {
		return false
	}
	// Write path + NUL to the persistent process.
	if _, err := g.stdin.Write(append([]byte(path), 0)); err != nil {
		// E-PENPAL-SCAN: disable checker on write failure to prevent desync.
		g.isGitRepo = false
		return false
	}
	// Read 4 NUL-terminated fields: source, linenum, pattern, pathname.
	var source string
	for i := 0; i < 4; i++ {
		if !g.scanner.Scan() {
			// E-PENPAL-SCAN: partial read leaves stream out of sync; disable.
			g.isGitRepo = false
			return false
		}
		if i == 0 {
			source = g.scanner.Text()
		}
	}
	// Non-empty source means a gitignore rule matched.
	return source != ""
}

func (g *gitIgnoreChecker) Close() {
	if g.stdin != nil {
		g.stdin.Close()
	}
	if g.cmd != nil {
		g.cmd.Wait()
	}
}

// scanNul is a bufio.SplitFunc that splits on NUL bytes.
func scanNul(data []byte, atEOF bool) (advance int, token []byte, err error) {
	if atEOF && len(data) == 0 {
		return 0, nil, nil
	}
	if i := bytes.IndexByte(data, 0); i >= 0 {
		return i + 1, data[:i], nil
	}
	if atEOF {
		return len(data), data, nil
	}
	return 0, nil, nil
}

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
	// projectScanned tracks which projects have had a full file scan
	projectScanned map[string]bool
}

// New creates a new cache
func New() *Cache {
	return &Cache{
		projectFiles:   make(map[string][]FileInfo),
		projectScanned: make(map[string]bool),
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
// E-PENPAL-PATH-MATCH: longest-prefix matching across all project root paths.
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
// E-PENPAL-PATH-MATCH: extends FindProjectByPath to check non-main worktree paths.
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
	c.projectScanned[projectName] = true
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

// AllFiles returns all files across all projects, sorted by modification time.
// Files are deduplicated by project+path: when a file appears in both a typed
// source and __all_markdown__, only the typed-source entry is kept.
// E-PENPAL-SRC-ALL-MD: dedup prevents __all_markdown__ from doubling the list.
func (c *Cache) AllFiles(limit int) []FileInfo {
	c.mu.RLock()
	defer c.mu.RUnlock()

	type fileKey struct {
		project, path string
	}
	best := make(map[fileKey]FileInfo)
	for _, files := range c.projectFiles {
		for _, f := range files {
			k := fileKey{f.Project, f.FullPath}
			if existing, ok := best[k]; ok {
				// Prefer typed-source entry over __all_markdown__
				if existing.Source == "__all_markdown__" && f.Source != "__all_markdown__" {
					best[k] = f
				}
			} else {
				best[k] = f
			}
		}
	}

	all := make([]FileInfo, 0, len(best))
	for _, f := range best {
		all = append(all, f)
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

// IsProjectScanned returns whether a project has had a full file scan.
func (c *Cache) IsProjectScanned(projectName string) bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.projectScanned[projectName]
}

// EnsureProjectScanned triggers a full file scan for a project if it hasn't
// been scanned yet. Returns true if a scan was actually performed (first call
// for this project). This is the lazy-scan entry point — called when a user
// first opens a project, rather than eagerly at startup.
// E-PENPAL-SCAN: lazy scan with write-lock gating to prevent duplicate walks.
func (c *Cache) EnsureProjectScanned(projectName string) bool {
	c.mu.Lock()
	if c.projectScanned[projectName] {
		c.mu.Unlock()
		return false
	}
	// Mark as in-progress under write lock to prevent concurrent scans.
	c.projectScanned[projectName] = true
	c.mu.Unlock()

	c.RefreshProject(projectName)
	return true
}

// RefreshProject rescans a single project's files across all its sources.
// projectName should be the qualified name (e.g., "Development/penpal").
// E-PENPAL-CACHE: walks filesystem and updates per-project file list and metadata.
func (c *Cache) RefreshProject(projectName string) {
	project := c.FindProject(projectName)
	if project == nil {
		return
	}

	files := filterManualFileInfos(project, scanProjectSources(project))
	c.SetProjectFiles(projectName, files)

	// Update project metadata
	c.mu.Lock()
	defer c.mu.Unlock()
	c.projectScanned[projectName] = true
	for i := range c.projects {
		if c.projects[i].QualifiedName() == projectName {
			c.projects[i].HasFiles = len(files) > 0
			if len(files) > 0 {
				c.projects[i].LastModified = files[0].ModTime
			}
			break
		}
	}
}

// RefreshAllProjects rescans all projects' files and updates metadata.
// E-PENPAL-CACHE: parallel refresh with concurrency limit of 4.
func (c *Cache) RefreshAllProjects() {
	projects := c.Projects()
	var wg sync.WaitGroup
	sem := make(chan struct{}, 4)
	for _, p := range projects {
		wg.Add(1)
		go func(qn string) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			c.RefreshProject(qn)
		}(p.QualifiedName())
	}
	wg.Wait()
}

// errFoundMarkdown is a sentinel used to short-circuit filepath.WalkDir
// once a .md file is found.
var errFoundMarkdown = errors.New("found markdown")

// CheckAllProjectsHasFiles does a cheap per-project check to set HasFiles
// without doing a full file scan. For each project, it walks the project
// root and stops as soon as it finds any .md file.
// E-PENPAL-SCAN: lightweight startup check with concurrency limit of 4.
func (c *Cache) CheckAllProjectsHasFiles() {
	projects := c.Projects()
	var wg sync.WaitGroup
	sem := make(chan struct{}, 4)
	for _, p := range projects {
		wg.Add(1)
		go func(p discovery.Project) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			found := projectHasAnyMarkdown(p.Path)
			c.mu.Lock()
			for i := range c.projects {
				if c.projects[i].QualifiedName() == p.QualifiedName() {
					c.projects[i].HasFiles = found
					break
				}
			}
			c.mu.Unlock()
		}(p)
	}
	wg.Wait()
}

// projectHasAnyMarkdown walks the directory tree and returns true as soon as
// it finds any .md file. Skips .git, node_modules, .hg, .svn, and nested
// worktrees/submodules. Does NOT use git check-ignore — a false positive
// from a .md file in a gitignored directory is harmless since the full scan
// on first access applies proper filtering.
// E-PENPAL-SCAN: lightweight startup check — no subprocess spawned.
func projectHasAnyMarkdown(projectPath string) bool {
	err := filepath.WalkDir(projectPath, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			name := d.Name()
			if name == ".git" || name == "node_modules" || name == ".hg" || name == ".svn" {
				return filepath.SkipDir
			}
			// Skip nested git worktrees and submodules (.git file, not dir).
			if path != projectPath {
				gitEntry := filepath.Join(path, ".git")
				if fi, err := os.Lstat(gitEntry); err == nil && !fi.IsDir() {
					return filepath.SkipDir
				}
			}
			return nil
		}
		if strings.HasSuffix(d.Name(), ".md") {
			return errFoundMarkdown
		}
		return nil
	})
	return errors.Is(err, errFoundMarkdown)
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
// E-PENPAL-CACHE: replaces the project list while preserving git enrichment.
func (c *Cache) RescanWith(projects []discovery.Project) {
	// Snapshot current state before replacing
	c.mu.RLock()
	existing := make(map[string]discovery.Project)
	existingScanned := make(map[string]bool)
	existingFiles := make(map[string][]FileInfo)
	for _, p := range c.projects {
		qn := p.QualifiedName()
		existing[qn] = p
		existingScanned[qn] = c.projectScanned[qn]
		if files, ok := c.projectFiles[qn]; ok {
			existingFiles[qn] = files
		}
	}
	c.mu.RUnlock()

	// Preserve git enrichment only when the project path hasn't changed.
	// If the path changed, Git metadata would be stale and enrichGitInfo
	// needs to re-discover it (it skips projects where Git != nil).
	for i := range projects {
		if prev, ok := existing[projects[i].QualifiedName()]; ok && prev.Path == projects[i].Path {
			projects[i].Git = prev.Git
		}
	}

	c.SetProjects(projects)

	// Determine which projects need scanning
	var toScan []string
	newNames := make(map[string]bool, len(projects))
	for _, p := range projects {
		qn := p.QualifiedName()
		newNames[qn] = true

		prev, existed := existing[qn]
		if !existed {
			// New project: needs scan
			toScan = append(toScan, qn)
		} else if !existingScanned[qn] {
			// Existed but never scanned: needs scan
			toScan = append(toScan, qn)
		} else if SourcesChanged(prev.Sources, p.Sources) {
			// Sources changed: needs rescan
			toScan = append(toScan, qn)
		} else {
			// Unchanged: preserve cached files
			c.mu.Lock()
			c.projectFiles[qn] = existingFiles[qn]
			c.projectScanned[qn] = true
			c.updateProjectMetadataLocked(qn, existingFiles[qn])
			c.mu.Unlock()
		}
	}

	// Clean up removed projects
	c.mu.Lock()
	for name := range existingFiles {
		if !newNames[name] {
			delete(c.projectFiles, name)
			delete(c.projectScanned, name)
		}
	}
	c.mu.Unlock()

	// Scan only the projects that need it, with concurrency limit
	if len(toScan) > 0 {
		var wg sync.WaitGroup
		sem := make(chan struct{}, 4)
		for _, qn := range toScan {
			wg.Add(1)
			go func(qn string) {
				defer wg.Done()
				sem <- struct{}{}
				defer func() { <-sem }()
				c.RefreshProject(qn)
			}(qn)
		}
		wg.Wait()
	}
}

// SourcesChanged returns true if two source lists differ materially.
// E-PENPAL-CACHE: used by RescanWith to detect which projects need rescanning.
func SourcesChanged(a, b []discovery.FileSource) bool {
	if len(a) != len(b) {
		return true
	}
	for i := range a {
		if a[i].Name != b[i].Name || a[i].Type != b[i].Type ||
			a[i].RootPath != b[i].RootPath || a[i].SourceTypeName != b[i].SourceTypeName {
			return true
		}
		if len(a[i].Files) != len(b[i].Files) {
			return true
		}
		for j := range a[i].Files {
			if a[i].Files[j] != b[i].Files[j] {
				return true
			}
		}
	}
	return false
}

// EnrichTitles fills in missing Title fields for files in the given project.
// This is called on demand when a user views a project, so titles appear
// immediately without waiting for a full background rescan.
// E-PENPAL-TITLE-EXTRACT: reads first 20 lines for H1 headings.
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
// E-PENPAL-TITLE-EXTRACT: scans first 20 lines for an H1 heading.
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

// ResolveFileInfo resolves source membership for a single absolute .md file path
// within a project. It applies the same source-priority, SkipDirs, RequireSibling,
// and ClassifyFile rules as scanProjectSources but without walking the filesystem
// or spawning a git check-ignore process. Returns FileInfo entries for each source
// that claims the file (typically one typed source + __all_markdown__). Returns nil
// if no source claims the file.
// E-PENPAL-SCAN: single-file source resolution for incremental cache updates.
// ResolveFileInfo resolves source membership for a single absolute .md file path
// without a filesystem walk. It applies the same exclusion rules as
// scanProjectSources: nested git worktree/submodule detection, gitignore
// ancestor-directory checks (P-PENPAL-SRC-GITIGNORE), SkipDirs filtering, and
// RequireSibling validation.
func ResolveFileInfo(project *discovery.Project, absPath string) []FileInfo {
	if !strings.HasSuffix(absPath, ".md") {
		return nil
	}

	info, err := os.Stat(absPath)
	if err != nil {
		return nil
	}
	if info.IsDir() {
		return nil
	}

	relToProject, err := filepath.Rel(project.Path, absPath)
	if err != nil || strings.HasPrefix(relToProject, "..") {
		return nil
	}

	title := extractTitle(absPath)
	var results []FileInfo
	typedClaimed := false

	for _, source := range project.Sources {
		isAllMarkdown := source.Name == "__all_markdown__"

		if source.Type == "thoughts" || source.Type == "tree" {
			rootPath := source.RootPath
			if rootPath == "" {
				continue
			}

			// Check containment
			if !strings.HasPrefix(absPath, rootPath+"/") && absPath != rootPath {
				continue
			}

			// Skip files under nested git worktrees/submodules.
			if isUnderNestedGitRepo(absPath, rootPath) {
				continue
			}

			// P-PENPAL-SRC-GITIGNORE: skip files whose ancestor directory
			// is gitignored (source root itself is exempt).
			if isAncestorDirGitIgnored(absPath, rootPath, project.Path) {
				continue
			}

			relToSource, err := filepath.Rel(rootPath, absPath)
			if err != nil {
				continue
			}

			// Check SkipDirs against each path component
			stName := source.SourceTypeName
			if stName == "" {
				stName = source.Name
			}
			st := discovery.GetSourceType(stName)

			if !isAllMarkdown && typedClaimed {
				continue // already claimed by an earlier typed source
			}

			if hasSkippedDir(relToSource, st) {
				continue
			}

			// RequireSibling check
			if st != nil && st.RequireSibling != "" {
				siblingPath := filepath.Join(filepath.Dir(absPath), st.RequireSibling)
				if _, err := os.Stat(siblingPath); err != nil {
					continue
				}
			}

			// ClassifyFile
			fileType := "other"
			if st != nil && st.ClassifyFile != nil {
				fileType = st.ClassifyFile(relToSource)
				if fileType == "" {
					continue // skip this file for this source
				}
			} else {
				if strings.Contains(relToSource, "research") {
					fileType = "research"
				} else if strings.Contains(relToSource, "plan") {
					fileType = "plan"
				}
			}

			if !isAllMarkdown {
				typedClaimed = true
			}

			results = append(results, FileInfo{
				Project:     project.QualifiedName(),
				Workspace:   project.WorkspaceName,
				ProjectPath: project.Path,
				Source:      source.Name,
				SourceType:  source.Type,
				SourceAuto:  source.Auto,
				Path:        relToSource,
				FullPath:    relToProject,
				Name:        filepath.Base(absPath),
				Title:       title,
				ModTime:     info.ModTime(),
				FileType:    fileType,
			})

		} else if source.Type == "files" {
			if !isAllMarkdown && typedClaimed {
				continue
			}
			found := false
			for _, f := range source.Files {
				if f == absPath {
					found = true
					break
				}
			}
			if !found {
				continue
			}

			fileType := "other"
			lower := strings.ToLower(filepath.Base(absPath))
			if strings.Contains(lower, "research") {
				fileType = "research"
			} else if strings.Contains(lower, "plan") {
				fileType = "plan"
			}

			if !isAllMarkdown {
				typedClaimed = true
			}

			results = append(results, FileInfo{
				Project:     project.QualifiedName(),
				Workspace:   project.WorkspaceName,
				ProjectPath: project.Path,
				Source:      source.Name,
				SourceType:  source.Type,
				SourceAuto:  source.Auto,
				Path:        filepath.Base(absPath),
				FullPath:    relToProject,
				Name:        filepath.Base(absPath),
				Title:       title,
				ModTime:     info.ModTime(),
				FileType:    fileType,
			})
		}
	}

	return results
}

// hasSkippedDir checks whether any directory component between the source root
// and the file matches the source type's SkipDirs.
func hasSkippedDir(relToSource string, st *discovery.SourceType) bool {
	if st == nil || len(st.SkipDirs) == 0 {
		return false
	}
	dir := filepath.Dir(relToSource)
	if dir == "." {
		return false
	}
	for _, component := range strings.Split(dir, string(filepath.Separator)) {
		if st.SkipDirs[component] {
			return true
		}
	}
	return false
}

// isUnderNestedGitRepo walks parent directories from absPath up to (but not
// including) rootPath, returning true if any intermediate directory contains a
// .git file (not directory), indicating a nested git worktree or submodule.
// This mirrors the nested-repo check in scanProjectSources without spawning
// a subprocess.
func isUnderNestedGitRepo(absPath, rootPath string) bool {
	dir := filepath.Dir(absPath)
	for dir != rootPath && strings.HasPrefix(dir, rootPath+"/") {
		gitEntry := filepath.Join(dir, ".git")
		if fi, err := os.Lstat(gitEntry); err == nil && !fi.IsDir() {
			return true
		}
		dir = filepath.Dir(dir)
	}
	return false
}

// isAncestorDirGitIgnored walks parent directories from absPath up to (but not
// including) rootPath, running a one-shot `git check-ignore -q` on each.
// Returns true if any ancestor directory is gitignored.
// P-PENPAL-SRC-GITIGNORE: the source root itself is exempt (always scanned).
func isAncestorDirGitIgnored(absPath, rootPath, projectPath string) bool {
	dir := filepath.Dir(absPath)
	for dir != rootPath && strings.HasPrefix(dir, rootPath+"/") {
		cmd := exec.Command("git", "-C", projectPath, "check-ignore", "-q", dir)
		if cmd.Run() == nil {
			return true // exit code 0 means ignored
		}
		dir = filepath.Dir(dir)
	}
	return false
}

// UpsertFile adds or updates file entries in the cache for the given absolute path.
// For existing entries (matched by FullPath), re-stats for ModTime and re-extracts
// Title. For new files, resolves source membership and inserts.
// E-PENPAL-CACHE: incremental cache mutation without filesystem walk.
func (c *Cache) UpsertFile(projectName string, project *discovery.Project, absPath string) bool {
	// Perform all filesystem and git I/O outside the lock.
	relToProject, err := filepath.Rel(project.Path, absPath)
	if err != nil {
		return false
	}

	info, err := os.Stat(absPath)
	if err != nil {
		return false
	}

	title := extractTitle(absPath)
	resolved := filterManualFileInfos(project, ResolveFileInfo(project, absPath))

	// Acquire lock only for the short critical section that mutates the cache.
	c.mu.Lock()
	defer c.mu.Unlock()

	files := c.projectFiles[projectName]

	// Check if any entries already exist for this path
	updated := false
	for i := range files {
		if files[i].FullPath == relToProject {
			files[i].ModTime = info.ModTime()
			files[i].Title = title
			updated = true
		}
	}

	if updated {
		sort.Slice(files, func(i, j int) bool {
			return files[i].ModTime.After(files[j].ModTime)
		})
		c.projectFiles[projectName] = files
		c.updateProjectMetadataLocked(projectName, files)
		return true
	}

	// New file — use pre-resolved source membership
	if len(resolved) == 0 {
		return false
	}

	files = append(files, resolved...)
	sort.Slice(files, func(i, j int) bool {
		return files[i].ModTime.After(files[j].ModTime)
	})
	c.projectFiles[projectName] = files
	c.updateProjectMetadataLocked(projectName, files)
	return true
}

func filterManualFileInfos(project *discovery.Project, files []FileInfo) []FileInfo {
	manualSources := make(map[string]bool)
	for _, source := range project.Sources {
		if source.SourceTypeName == "manual" {
			manualSources[source.Name] = true
		}
	}
	if len(manualSources) == 0 {
		return files
	}
	filtered := make([]FileInfo, 0, len(files))
	for _, file := range files {
		if manualSources[file.Source] {
			continue
		}
		filtered = append(filtered, file)
	}
	return filtered
}

// RemoveFile removes all cache entries with the given project-relative path.
// E-PENPAL-CACHE: incremental cache mutation without filesystem walk.
func (c *Cache) RemoveFile(projectName, fullPath string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()

	files := c.projectFiles[projectName]
	if files == nil {
		return false
	}

	n := 0
	for _, f := range files {
		if f.FullPath != fullPath {
			files[n] = f
			n++
		}
	}
	if n == len(files) {
		return false // nothing removed
	}

	files = files[:n]
	c.projectFiles[projectName] = files
	c.updateProjectMetadataLocked(projectName, files)
	return true
}

// updateProjectMetadataLocked updates HasFiles and LastModified for a project.
// Must be called with c.mu held for writing.
func (c *Cache) updateProjectMetadataLocked(projectName string, files []FileInfo) {
	for i := range c.projects {
		if c.projects[i].QualifiedName() == projectName {
			c.projects[i].HasFiles = len(files) > 0
			if len(files) > 0 {
				c.projects[i].LastModified = files[0].ModTime
			}
			break
		}
	}
}

// ScanProjectSourcesForWorktree scans a project's sources remapped to a worktree path.
// Each source's RootPath under the project is remapped to the equivalent path under
// the worktree. Sources whose directory doesn't exist in the worktree are skipped.
// E-PENPAL-SCAN: remaps source paths to a worktree and delegates to scanProjectSources.
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
// Each source does its own walk. Files are de-duplicated by project-relative
// path: if multiple sources cover the same file, only the first source's entry
// is kept — except __all_markdown__ which claims every file regardless.
// E-PENPAL-SCAN: per-source WalkDir, classifies files, deduplicates, sorts by ModTime.
func scanProjectSources(project *discovery.Project) []FileInfo {
	var files []FileInfo
	seen := make(map[string]bool) // project-relative paths already claimed
	gitChecker := newGitIgnoreChecker(project.Path)
	defer gitChecker.Close()

	for _, source := range project.Sources {
		if source.Type == "thoughts" || source.Type == "tree" {
			rootPath := source.RootPath
			if rootPath == "" {
				continue
			}

			stName := source.SourceTypeName
			if stName == "" {
				stName = source.Name
			}
			st := discovery.GetSourceType(stName)
			isAllMarkdown := source.Name == "__all_markdown__"

			filepath.WalkDir(rootPath, func(path string, d fs.DirEntry, err error) error {
				if err != nil {
					return nil
				}
				if d.IsDir() {
					// Skip nested git worktrees and submodules: they contain a
					// .git file (not directory) pointing at the real gitdir.
					if path != rootPath {
						gitEntry := filepath.Join(path, ".git")
						if fi, err := os.Lstat(gitEntry); err == nil && !fi.IsDir() {
							return filepath.SkipDir
						}
					}
					if d.Name() == ".git" {
						return filepath.SkipDir
					}
					// P-PENPAL-SRC-GITIGNORE: registered source roots are
					// always scanned even if gitignored.
					if path != rootPath && gitChecker.IsIgnored(path) {
						return filepath.SkipDir
					}
					if st != nil && st.SkipDirs[d.Name()] {
						return filepath.SkipDir
					}
					return nil
				}
				if !strings.HasSuffix(path, ".md") {
					return nil
				}
				// E-PENPAL-SOURCE-REGISTRY: RequireSibling pre-filter.
				if st != nil && st.RequireSibling != "" {
					siblingPath := filepath.Join(filepath.Dir(path), st.RequireSibling)
					if _, err := os.Stat(siblingPath); err != nil {
						return nil
					}
				}
				relToSource, _ := filepath.Rel(rootPath, path)
				relToProject, _ := filepath.Rel(project.Path, path)

				if !isAllMarkdown && seen[relToProject] {
					return nil // already claimed by an earlier source
				}

				fileType := "other"
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

				// Only stat .md files (for ModTime), not every entry.
				info, err := d.Info()
				if err != nil {
					return nil
				}

				if !isAllMarkdown {
					seen[relToProject] = true
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
					Name:        d.Name(),
					Title:       extractTitle(path),
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
