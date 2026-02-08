package server

import (
	"bufio"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/loganj/birdseye/internal/discovery"
)

// MatchedFile represents a file that matched by name or content
type MatchedFile struct {
	Path      string // relative to project root (e.g., "thoughts/plans/foo.md")
	Name      string
	NameMatch bool   // matched by filename
	FileType  string // "research", "plan", or "other"
}

// ProjectResults groups matched files for a single project
type ProjectResults struct {
	Project       string
	QualifiedName string
	Workspace     string
	ProjectPath   string // absolute filesystem path
	Files         []MatchedFile
}

// SearchData is passed to the search template
type SearchData struct {
	Query            string
	MatchingProjects []discovery.Project
	ProjectResults   []ProjectResults
	TotalFiles       int
}

func (s *Server) handleSearch(w http.ResponseWriter, r *http.Request) {
	query := strings.ToLower(strings.TrimSpace(r.URL.Query().Get("q")))

	if query == "" {
		s.getTemplate().ExecuteTemplate(w, "search.html", SearchData{})
		return
	}

	var matchingProjects []discovery.Project
	projectResultsMap := make(map[string]*ProjectResults)
	totalFiles := 0
	const maxFiles = 100

	for _, project := range s.cache.Projects() {
		// Check if project name matches
		if strings.Contains(strings.ToLower(project.Name), query) {
			matchingProjects = append(matchingProjects, project)
		}

		// Track which files matched, deduped by project-relative path
		fileMatches := make(map[string]*MatchedFile)

		// Search files across all sources in this project
		for _, source := range project.Sources {
			if source.RootPath == "" {
				continue
			}

			filepath.Walk(source.RootPath, func(path string, info os.FileInfo, err error) error {
				if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
					return nil
				}

				relToProject, _ := filepath.Rel(project.Path, path)
				fileName := filepath.Base(path)

				fileType := classifyFile(relToProject)

				// Check if filename matches
				if strings.Contains(strings.ToLower(fileName), query) {
					fileMatches[relToProject] = &MatchedFile{
						Path:      relToProject,
						Name:      fileName,
						NameMatch: true,
						FileType:  fileType,
					}
				}

				// Search file content
				file, err := os.Open(path)
				if err != nil {
					return nil
				}
				defer file.Close()

				scanner := bufio.NewScanner(file)
				for scanner.Scan() {
					if strings.Contains(strings.ToLower(scanner.Text()), query) {
						if _, ok := fileMatches[relToProject]; !ok {
							fileMatches[relToProject] = &MatchedFile{
								Path:     relToProject,
								Name:     fileName,
								FileType: fileType,
							}
						}
						break // only need to know it matched, not how many times
					}
				}
				return nil
			})
		}

		// Collect matches for this project
		if len(fileMatches) > 0 {
			qn := project.QualifiedName()
			pr := &ProjectResults{Project: project.Name, Workspace: project.WorkspaceName, ProjectPath: project.Path, QualifiedName: qn}
			for _, m := range fileMatches {
				pr.Files = append(pr.Files, *m)
				totalFiles++
			}
			// Sort: name matches first, then alphabetically
			sort.Slice(pr.Files, func(i, j int) bool {
				if pr.Files[i].NameMatch != pr.Files[j].NameMatch {
					return pr.Files[i].NameMatch
				}
				return pr.Files[i].Name < pr.Files[j].Name
			})
			projectResultsMap[qn] = pr
		}

		if totalFiles >= maxFiles {
			break
		}
	}

	// Convert map to sorted slice
	var projectResults []ProjectResults
	for _, pr := range projectResultsMap {
		projectResults = append(projectResults, *pr)
	}
	sort.Slice(projectResults, func(i, j int) bool {
		return projectResults[i].Project < projectResults[j].Project
	})

	data := SearchData{
		Query:            query,
		MatchingProjects: matchingProjects,
		ProjectResults:   projectResults,
		TotalFiles:       totalFiles,
	}
	s.getTemplate().ExecuteTemplate(w, "search.html", data)
}

func classifyFile(relPath string) string {
	lower := strings.ToLower(relPath)
	if strings.Contains(lower, "research") {
		return "research"
	}
	if strings.Contains(lower, "plan") {
		return "plan"
	}
	return "other"
}

func truncate(s string, max int) string {
	s = strings.TrimSpace(s)
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}
