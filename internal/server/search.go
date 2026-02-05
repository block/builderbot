package server

import (
	"bufio"
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

type SearchResult struct {
	Project  string
	FilePath string
	FileName string
	Line     int
	Context  string
}

func (s *Server) handleSearch(w http.ResponseWriter, r *http.Request) {
	query := strings.ToLower(strings.TrimSpace(r.URL.Query().Get("q")))

	if query == "" {
		data := struct {
			Query   string
			Results []SearchResult
		}{
			Query:   "",
			Results: nil,
		}
		s.tmpl.ExecuteTemplate(w, "search.html", data)
		return
	}

	var results []SearchResult
	for _, project := range s.projects {
		filepath.Walk(project.ThoughtsPath, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
				return nil
			}

			file, err := os.Open(path)
			if err != nil {
				return nil
			}
			defer file.Close()

			relPath, _ := filepath.Rel(project.ThoughtsPath, path)
			scanner := bufio.NewScanner(file)
			lineNum := 0
			for scanner.Scan() {
				lineNum++
				line := scanner.Text()
				if strings.Contains(strings.ToLower(line), query) {
					results = append(results, SearchResult{
						Project:  project.Name,
						FilePath: relPath,
						FileName: filepath.Base(path),
						Line:     lineNum,
						Context:  truncate(line, 120),
					})
					if len(results) >= 100 {
						return filepath.SkipAll
					}
				}
			}
			return nil
		})
		if len(results) >= 100 {
			break
		}
	}

	data := struct {
		Query   string
		Results []SearchResult
	}{
		Query:   query,
		Results: results,
	}
	s.tmpl.ExecuteTemplate(w, "search.html", data)
}

func truncate(s string, max int) string {
	s = strings.TrimSpace(s)
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}
