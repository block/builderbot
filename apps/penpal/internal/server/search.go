package server

import (
	"bufio"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/loganj/penpal/internal/discovery"
)

// handleAPISearch returns search results as JSON for the React frontend.
// E-PENPAL-SEARCH: matches project names, filenames, content; capped at 100.
func (s *Server) handleAPISearch(w http.ResponseWriter, r *http.Request) {
	query := strings.ToLower(strings.TrimSpace(r.URL.Query().Get("q")))

	type apiMatchedFile struct {
		Path      string `json:"path"`
		Name      string `json:"name"`
		Title     string `json:"title,omitempty"`
		NameMatch bool   `json:"nameMatch,omitempty"`
		FileType  string `json:"fileType"`
	}
	type apiProjectResults struct {
		Project       string           `json:"project"`
		QualifiedName string           `json:"qualifiedName"`
		Workspace     string           `json:"workspace,omitempty"`
		ProjectPath   string           `json:"projectPath"`
		Files         []apiMatchedFile `json:"files"`
	}
	type apiSearchResponse struct {
		Query            string              `json:"query"`
		MatchingProjects []apiProjectResults `json:"matchingProjects,omitempty"`
		ProjectResults   []apiProjectResults `json:"projectResults,omitempty"`
		TotalFiles       int                 `json:"totalFiles"`
	}

	if query == "" {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(apiSearchResponse{})
		return
	}

	var matchingProjects []apiProjectResults
	projectResultsMap := make(map[string]*apiProjectResults)
	totalFiles := 0
	const maxFiles = 100

	for _, project := range s.cache.Projects() {
		qn := project.QualifiedName()

		if strings.Contains(strings.ToLower(project.Name), query) {
			matchingProjects = append(matchingProjects, apiProjectResults{
				Project:       project.Name,
				QualifiedName: qn,
				Workspace:     project.WorkspaceName,
				ProjectPath:   project.Path,
			})
		}

		fileMatches := make(map[string]*apiMatchedFile)

		for _, source := range project.Sources {
			if source.RootPath == "" {
				continue
			}

			stName := source.SourceTypeName
			if stName == "" {
				stName = source.Name
			}
			st := discovery.GetSourceType(stName)

			filepath.Walk(source.RootPath, func(path string, info os.FileInfo, err error) error {
				if err != nil {
					return nil
				}
				if info.IsDir() {
					name := info.Name()
					// E-PENPAL-SEARCH: skip .git dirs and nested worktrees/submodules.
					if name == ".git" {
						return filepath.SkipDir
					}
					if path != source.RootPath {
						gitEntry := filepath.Join(path, ".git")
						if fi, gErr := os.Lstat(gitEntry); gErr == nil && !fi.IsDir() {
							return filepath.SkipDir
						}
					}
					if st != nil && st.SkipDirs[name] {
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

				relToProject, _ := filepath.Rel(project.Path, path)
				relToSource, _ := filepath.Rel(source.RootPath, path)
				fileName := filepath.Base(path)

				fileType := classifyFile(relToProject)
				if st != nil && st.ClassifyFile != nil {
					fileType = st.ClassifyFile(relToSource)
					if fileType == "" {
						return nil // skip files not recognized by this source type
					}
				}

				if strings.Contains(strings.ToLower(fileName), query) {
					fileMatches[relToProject] = &apiMatchedFile{
						Path:      relToProject,
						Name:      fileName,
						NameMatch: true,
						FileType:  fileType,
					}
				}

				file, err := os.Open(path)
				if err != nil {
					return nil
				}
				defer file.Close()

				scanner := bufio.NewScanner(file)
				for scanner.Scan() {
					if strings.Contains(strings.ToLower(scanner.Text()), query) {
						if _, ok := fileMatches[relToProject]; !ok {
							fileMatches[relToProject] = &apiMatchedFile{
								Path:     relToProject,
								Name:     fileName,
								FileType: fileType,
							}
						}
						break
					}
				}
				return nil
			})
		}

		if len(fileMatches) > 0 {
			pr := &apiProjectResults{Project: project.Name, Workspace: project.WorkspaceName, ProjectPath: project.Path, QualifiedName: qn}
			for _, m := range fileMatches {
				if cf := s.cache.FindFile(qn, m.Path); cf != nil && cf.Title != "" {
					m.Title = cf.Title
				}
				pr.Files = append(pr.Files, *m)
				totalFiles++
			}
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

	var projectResults []apiProjectResults
	for _, pr := range projectResultsMap {
		projectResults = append(projectResults, *pr)
	}
	sort.Slice(projectResults, func(i, j int) bool {
		return projectResults[i].Project < projectResults[j].Project
	})

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(apiSearchResponse{
		Query:            query,
		MatchingProjects: matchingProjects,
		ProjectResults:   projectResults,
		TotalFiles:       totalFiles,
	})
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
