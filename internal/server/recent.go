package server

import (
	"net/http"
	"time"
)

type RecentFile struct {
	Project  string
	FilePath string
	FileName string
	ModTime  time.Time
	Age      string
	FileType string
}

func (s *Server) handleRecent(w http.ResponseWriter, r *http.Request) {
	// Get files from cache
	cachedFiles := s.cache.AllFiles(50)
	files := make([]RecentFile, len(cachedFiles))
	for i, f := range cachedFiles {
		files[i] = RecentFile{
			Project:  f.Project,
			FilePath: f.FullPath,
			FileName: f.Name,
			ModTime:  f.ModTime,
			Age:      formatAge(f.ModTime),
			FileType: f.FileType,
		}
	}

	nav := s.buildNav("")
	pageData := struct {
		Files []RecentFile
	}{
		Files: files,
	}
	s.renderPage(w, "recent.html", nav, pageData)
}
