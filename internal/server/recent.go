package server

import (
	"net/http"
	"time"
)

type RecentFile struct {
	Project   string
	Workspace string
	FilePath  string
	FileName  string
	ModTime   time.Time
	Age       string
}

func (s *Server) handleRecent(w http.ResponseWriter, r *http.Request) {
	// Get files from cache
	cachedFiles := s.cache.AllFiles(50)
	files := make([]RecentFile, len(cachedFiles))
	for i, f := range cachedFiles {
		files[i] = RecentFile{
			Project:   f.Project,
			Workspace: f.Workspace,
			FilePath:  f.FullPath,
			FileName:  f.Name,
			ModTime:   f.ModTime,
			Age:       formatAge(f.ModTime),
		}
	}

	data := struct {
		Files []RecentFile
	}{
		Files: files,
	}
	s.getTemplate().ExecuteTemplate(w, "recent.html", data)
}
