package discovery

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/loganj/birdseye/internal/config"
)

// SourceType defines a pluggable source type with metadata for discovery,
// UI rendering, and file classification.
type SourceType struct {
	Name             string                           // unique identifier (e.g., "thoughts", "rp1")
	DisplayName      string                           // badge text (e.g., "RPI", "RP1")
	BadgeColor       string                           // CSS color for badge text
	BadgeBg          string                           // CSS background for badge
	BadgeActiveBg    string                           // CSS background when sidebar item is active
	BadgeActiveColor string                           // CSS color when sidebar item is active
	AutoDetectDir    string                           // directory name to look for (e.g., "thoughts", ".rp1")
	ScanMode         string                           // "tree" (walk for .md) or "files" (explicit list)
	DetectAtWSRoot   bool                             // also detect at workspace root level
	ClassifyFile     func(path string) string         // returns file type for a path within the source
	GroupFiles       func(paths []string) []FileGroup // optional: groups files for display; if nil, single flat list
	ShowDirHeadings  bool                             // show directory headings above files in each subdirectory
}

// FileGroup represents a named group of files within a source for display.
type FileGroup struct {
	Name  string   // display name for the group header (empty = no header)
	Paths []string // source-relative paths in display order
}

// Built-in source types, keyed by name
var sourceTypes = map[string]*SourceType{}

// sourceTypeOrder preserves registration order for stable iteration
var sourceTypeOrder []string

// RegisterSourceType adds a source type to the registry.
func RegisterSourceType(st *SourceType) {
	if _, exists := sourceTypes[st.Name]; !exists {
		sourceTypeOrder = append(sourceTypeOrder, st.Name)
	}
	sourceTypes[st.Name] = st
}

// GetSourceType returns a registered source type by name, or nil.
func GetSourceType(name string) *SourceType {
	return sourceTypes[name]
}

// AllSourceTypes returns all registered source types in registration order.
func AllSourceTypes() []*SourceType {
	result := make([]*SourceType, 0, len(sourceTypeOrder))
	for _, name := range sourceTypeOrder {
		result = append(result, sourceTypes[name])
	}
	return result
}

func init() {
	RegisterSourceType(&SourceType{
		Name:             "thoughts",
		DisplayName:      "RPI",
		BadgeColor:       "#888",
		BadgeBg:          "#f0f0f0",
		BadgeActiveBg:    "#d4e4fc",
		BadgeActiveColor: "#5a8fd8",
		AutoDetectDir:    "thoughts",
		ScanMode:         "tree",
		DetectAtWSRoot:   true,
		ClassifyFile: func(path string) string {
			if strings.Contains(path, "research") {
				return "research"
			} else if strings.Contains(path, "plan") {
				return "plan"
			}
			return "other"
		},
	})

	RegisterSourceType(&SourceType{
		Name:             "rp1",
		DisplayName:      "RP1",
		BadgeColor:       "#8b5cf6",
		BadgeBg:          "#f5f0ff",
		BadgeActiveBg:    "#ede5ff",
		BadgeActiveColor: "#7c3aed",
		AutoDetectDir:    ".rp1",
		ScanMode:         "tree",
		DetectAtWSRoot:   false,
		ClassifyFile: func(path string) string {
			switch {
			case strings.HasPrefix(path, "work/archives/"):
				return "" // hidden — archived features are not shown
			case strings.HasPrefix(path, "work/worktrees/"):
				return "" // hidden — git worktrees for agent execution
			case strings.HasPrefix(path, "work/notes/"):
				return "" // hidden — internal tooling notes
			case strings.HasPrefix(path, "context/"):
				return "knowledge"
			case strings.HasPrefix(path, "work/features/"):
				return classifyRP1Feature(path)
			case strings.HasPrefix(path, "work/quick-builds/"):
				return "quick"
			case strings.HasPrefix(path, "work/prds/"):
				return "prd"
			case strings.HasPrefix(path, "work/research/"):
				return "research"
			case strings.HasPrefix(path, "work/pr-reviews/"):
				return "review"
			case strings.HasPrefix(path, "work/content/"):
				return "content"
			case strings.HasPrefix(path, "work/issues/"):
				return classifyRP1Issue(path)
			case path == "work/charter.md":
				return "charter"
			case isRP1TopLevelReport(path):
				return "report"
			default:
				return "other"
			}
		},
		GroupFiles: groupRP1Paths,
	})

	RegisterSourceType(&SourceType{
		Name:            "manual",
		ShowDirHeadings: true,
	})
}

// classifyRP1Feature classifies a file under work/features/{id}/ by its filename.
func classifyRP1Feature(path string) string {
	base := filepath.Base(path)
	switch base {
	case "requirements.md":
		return "requirement"
	case "design.md":
		return "design"
	case "design-decisions.md":
		return "design"
	case "tasks.md":
		return "task"
	case "field-notes.md":
		return "field-notes"
	case "hypotheses.md":
		return "hypothesis"
	case "test_report.md":
		return "test-report"
	case "verification-report.md":
		return "verification"
	default:
		return "other"
	}
}

// classifyRP1Issue classifies a file under work/issues/{id}/ by its filename.
func classifyRP1Issue(path string) string {
	base := filepath.Base(path)
	switch base {
	case "investigation_report.md":
		return "investigation"
	case "root_cause_analysis.md":
		return "analysis"
	case "implementation_plan.md":
		return "plan"
	default:
		if strings.Contains(path, "/evidence/") {
			return "evidence"
		}
		return "other"
	}
}

// isRP1TopLevelReport returns true for known report files directly under work/.
func isRP1TopLevelReport(path string) bool {
	switch path {
	case "work/audit-report.md",
		"work/investigation-report.md",
		"work/security-report.md",
		"work/strategy-report.md",
		"work/project-overview.md":
		return true
	default:
		return false
	}
}

// Badge holds rendering metadata for a source type badge.
type Badge struct {
	Text        string
	Color       string
	Bg          string
	ActiveBg    string
	ActiveColor string
}

// FileSource represents a set of files to display for a project.
type FileSource struct {
	Name           string   // display name (e.g., "thoughts", "docs")
	Type           string   // "tree" or "files"
	SourceTypeName string   // registered SourceType name (e.g., "thoughts", "rp1", "manual")
	RootPath       string   // absolute path to tree root (for thoughts/tree types)
	Files          []string // absolute paths (for "files" type)
	Auto           bool     // true if auto-detected (thoughts/), false if user-added
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
	LastModified  time.Time
}

// ThoughtsPath returns the thoughts source root if present, empty string otherwise.
func (p *Project) ThoughtsPath() string {
	for _, s := range p.Sources {
		if s.Name == "thoughts" && s.Auto {
			return s.RootPath
		}
	}
	return ""
}

// HasThoughts returns true if the project has a thoughts/ directory.
func (p *Project) HasThoughts() bool {
	return p.ThoughtsPath() != ""
}

// HasSourceType returns true if the project has an auto-detected source of the given type.
func (p *Project) HasSourceType(name string) bool {
	for _, s := range p.Sources {
		if s.Name == name && s.Auto {
			return true
		}
	}
	return false
}

// Badges returns badge metadata for all auto-detected sources.
func (p *Project) Badges() []Badge {
	var badges []Badge
	for _, s := range p.Sources {
		if !s.Auto {
			continue
		}
		st := GetSourceType(s.Name)
		if st != nil {
			badges = append(badges, Badge{
				Text:        st.DisplayName,
				Color:       st.BadgeColor,
				Bg:          st.BadgeBg,
				ActiveBg:    st.BadgeActiveBg,
				ActiveColor: st.BadgeActiveColor,
			})
		}
	}
	return badges
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

// DetectSources finds auto-detectable file sources in a project directory
// by checking for all registered source types with an AutoDetectDir.
func DetectSources(projectPath string) []FileSource {
	var sources []FileSource
	for _, st := range AllSourceTypes() {
		if st.AutoDetectDir == "" {
			continue
		}
		dirPath := filepath.Join(projectPath, st.AutoDetectDir)
		if info, err := os.Stat(dirPath); err == nil && info.IsDir() {
			sources = append(sources, FileSource{
				Name:           st.Name,
				Type:           st.ScanMode,
				SourceTypeName: st.Name,
				RootPath:       dirPath,
				Auto:           true,
			})
		}
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

	// Also check for source types that can appear at workspace root level
	var rootSources []FileSource
	for _, st := range AllSourceTypes() {
		if st.AutoDetectDir == "" || !st.DetectAtWSRoot {
			continue
		}
		dirPath := filepath.Join(workspacePath, st.AutoDetectDir)
		if info, err := os.Stat(dirPath); err == nil && info.IsDir() {
			rootSources = append(rootSources, FileSource{
				Name:           st.Name,
				Type:           st.ScanMode,
				SourceTypeName: st.Name,
				RootPath:       dirPath,
				Auto:           true,
			})
		}
	}
	if len(rootSources) > 0 {
		projects = append(projects, Project{
			Name:          "(root)",
			Path:          workspacePath,
			Sources:       rootSources,
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
	project.Sources = append(project.Sources, SourceConfigsToFileSources(absPath, cfg.Sources)...)

	return project, nil
}

// SourceConfigsToFileSources converts config source entries to runtime FileSources.
func SourceConfigsToFileSources(projectPath string, configs []config.SourceConfig) []FileSource {
	var sources []FileSource
	for _, src := range configs {
		fs := FileSource{
			Name:           src.Name,
			Type:           src.Type,
			SourceTypeName: "manual",
			Auto:           false,
		}
		if src.Type == "tree" {
			fs.RootPath = filepath.Join(projectPath, src.Path)
			if fs.Name == "" {
				fs.Name = src.Path
			}
		} else if src.Type == "files" {
			fs.Files = make([]string, len(src.Files))
			for i, f := range src.Files {
				fs.Files[i] = filepath.Join(projectPath, f)
			}
			if fs.Name == "" {
				fs.Name = "files"
			}
		}
		sources = append(sources, fs)
	}
	return sources
}

// groupRP1Paths groups RP1 source-relative paths into ordered display groups.
func groupRP1Paths(paths []string) []FileGroup {
	categories := map[string][]string{}
	features := map[string][]string{}
	issues := map[string][]string{}

	for _, p := range paths {
		switch {
		case strings.HasPrefix(p, "context/"):
			categories["Context"] = append(categories["Context"], p)
		case strings.HasPrefix(p, "work/prds/"), p == "work/charter.md":
			categories["Blueprint"] = append(categories["Blueprint"], p)
		case strings.HasPrefix(p, "work/quick-builds/"):
			categories["Quick Builds"] = append(categories["Quick Builds"], p)
		case strings.HasPrefix(p, "work/research/"):
			categories["Research"] = append(categories["Research"], p)
		case strings.HasPrefix(p, "work/pr-reviews/"):
			categories["Reviews"] = append(categories["Reviews"], p)
		case strings.HasPrefix(p, "work/content/"):
			categories["Content"] = append(categories["Content"], p)
		case strings.HasPrefix(p, "work/issues/"):
			rest := p[len("work/issues/"):]
			parts := strings.SplitN(rest, "/", 2)
			if len(parts) == 2 && parts[0] != "" {
				issues[parts[0]] = append(issues[parts[0]], p)
			} else {
				categories["Other"] = append(categories["Other"], p)
			}
		case strings.HasPrefix(p, "work/features/"):
			rest := p[len("work/features/"):]
			parts := strings.SplitN(rest, "/", 2)
			if len(parts) == 2 && parts[0] != "" {
				features[parts[0]] = append(features[parts[0]], p)
			} else {
				categories["Other"] = append(categories["Other"], p)
			}
		default:
			categories["Other"] = append(categories["Other"], p)
		}
	}

	var groups []FileGroup

	// Fixed-order categories
	for _, cat := range []string{"Blueprint", "Quick Builds", "Research", "Reviews", "Content", "Other"} {
		if files, ok := categories[cat]; ok && len(files) > 0 {
			groups = append(groups, FileGroup{Name: cat, Paths: files})
		}
	}

	// Issues sorted alphabetically
	issueIDs := make([]string, 0, len(issues))
	for id := range issues {
		issueIDs = append(issueIDs, id)
	}
	sort.Strings(issueIDs)
	for _, id := range issueIDs {
		groups = append(groups, FileGroup{Name: "Issue: " + id, Paths: issues[id]})
	}

	// Features sorted alphabetically
	featureIDs := make([]string, 0, len(features))
	for id := range features {
		featureIDs = append(featureIDs, id)
	}
	sort.Strings(featureIDs)
	for _, id := range featureIDs {
		groups = append(groups, FileGroup{Name: "Feature: " + id, Paths: features[id]})
	}

	// Context last — least interesting for active work
	if files, ok := categories["Context"]; ok && len(files) > 0 {
		groups = append(groups, FileGroup{Name: "Context", Paths: files})
	}

	return groups
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
