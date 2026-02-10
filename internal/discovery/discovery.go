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
	Name             string                   // unique identifier (e.g., "thoughts", "rp1")
	DisplayName      string                   // badge text (e.g., "RPI", "RP1")
	BadgeColor       string                   // CSS color for badge text
	BadgeBg          string                   // CSS background for badge
	BadgeActiveBg    string                   // CSS background when sidebar item is active
	BadgeActiveColor string                   // CSS color when sidebar item is active
	AutoDetectDir    string                   // directory name to look for (e.g., "thoughts", ".rp1")
	ScanMode         string                   // "tree" (walk for .md) or "files" (explicit list)
	DetectAtWSRoot   bool                     // also detect at workspace root level
	ClassifyFile     func(path string) string // returns file type for a path within the source
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
			case strings.HasPrefix(path, "context/"):
				return "knowledge"
			case strings.HasPrefix(path, "work/features/"):
				return classifyRP1Feature(path)
			case strings.HasPrefix(path, "work/quick-builds/"):
				return "quick"
			case strings.HasPrefix(path, "work/prds/"):
				return "prd"
			case path == "work/charter.md":
				return "charter"
			default:
				return "other"
			}
		},
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
	case "tasks.md":
		return "task"
	case "field-notes.md":
		return "field-notes"
	default:
		return "other"
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
	Name     string   // display name (e.g., "thoughts", "docs")
	Type     string   // "tree" or "files"
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
				Name:     st.Name,
				Type:     st.ScanMode,
				RootPath: dirPath,
				Auto:     true,
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
				Name:     st.Name,
				Type:     st.ScanMode,
				RootPath: dirPath,
				Auto:     true,
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
			Name: src.Name,
			Type: src.Type,
			Auto: false,
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

// ExtractFeatureID extracts the feature ID from an RP1 feature file path.
// Returns empty string if not a feature file.
// Example: ".rp1/work/features/rp1-differentiation/requirements.md" -> "rp1-differentiation"
func ExtractFeatureID(fullPath, sourceName string) string {
	if sourceName != "rp1" {
		return ""
	}

	// Normalize path separators
	path := filepath.ToSlash(fullPath)

	// Match pattern: .rp1/work/features/{feature-id}/{file}
	const prefix = ".rp1/work/features/"
	if !strings.HasPrefix(path, prefix) {
		return ""
	}

	// Extract segment after prefix
	remainder := path[len(prefix):]
	parts := strings.SplitN(remainder, "/", 2)
	if len(parts) < 2 {
		return "" // malformed: no filename after feature-id
	}

	featureID := strings.TrimSpace(parts[0])
	if featureID == "" {
		return ""
	}

	return featureID
}

// DetectRP1Category categorizes non-feature RP1 files by path.
// Returns category name or empty string if not categorized.
func DetectRP1Category(fullPath, sourceName string) string {
	if sourceName != "rp1" {
		return ""
	}

	path := filepath.ToSlash(fullPath)

	switch {
	case strings.HasPrefix(path, ".rp1/context/"):
		return "Context"
	case strings.HasPrefix(path, ".rp1/work/prds/"):
		return "PRDs"
	case strings.HasPrefix(path, ".rp1/work/quick-builds/"):
		return "Quick Builds"
	case strings.HasPrefix(path, ".rp1/work/features/"):
		return "" // feature files, not categorized
	case strings.HasPrefix(path, ".rp1/work/archives/"):
		return "" // archived, not shown
	default:
		return "Other"
	}
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
