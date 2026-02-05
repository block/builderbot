package server

import (
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

type RecentFile struct {
	Project  string
	FilePath string
	FileName string
	ModTime  time.Time
}

func (s *Server) handleRecent(w http.ResponseWriter, r *http.Request) {
	var files []RecentFile

	for _, project := range s.projects {
		filepath.Walk(project.ThoughtsPath, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() || !strings.HasSuffix(path, ".md") {
				return nil
			}
			relPath, _ := filepath.Rel(project.ThoughtsPath, path)
			files = append(files, RecentFile{
				Project:  project.Name,
				FilePath: relPath,
				FileName: filepath.Base(path),
				ModTime:  info.ModTime(),
			})
			return nil
		})
	}

	sort.Slice(files, func(i, j int) bool {
		return files[i].ModTime.After(files[j].ModTime)
	})

	// Limit to 50 most recent
	if len(files) > 50 {
		files = files[:50]
	}

	data := struct {
		Files []RecentFile
	}{
		Files: files,
	}
	s.tmpl.ExecuteTemplate(w, "recent.html", data)
}
