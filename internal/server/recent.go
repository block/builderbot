package server

import (
	"net/http"
	"sort"
	"time"

	"github.com/loganj/penpal/internal/cache"
)

type RecentFile struct {
	Project      string
	FilePath     string
	FileName     string
	DisplayName  string
	ModTime      time.Time
	Age          string
	FileType     string
	ActivityType string // "viewed", "modified", "created", "comment", "published", or ""
	ActivityAge  string // "2m ago", "just now", etc.
}

func (s *Server) handleRecent(w http.ResponseWriter, r *http.Request) {
	files := s.mergeRecentFiles(50)

	nav := s.buildNav(r, "")
	nav.ActivePage = "recent"
	pageData := struct {
		Files []RecentFile
	}{
		Files: files,
	}
	s.renderPage(w, "recent.html", nav, pageData)
}

// mergeRecentFiles combines activity-tracked files with the filesystem file list.
// Files with tracked activity come first (sorted by activity timestamp),
// then files without activity (sorted by ModTime).
func (s *Server) mergeRecentFiles(limit int) []RecentFile {
	cachedFiles := s.cache.AllFiles(limit)

	// Build lookup of cached files by project+path
	type fileKey struct {
		project  string
		filePath string
	}
	cachedByKey := make(map[fileKey]*cache.FileInfo, len(cachedFiles))
	for i := range cachedFiles {
		f := &cachedFiles[i]
		cachedByKey[fileKey{f.Project, f.FullPath}] = f
	}

	// Get activity-tracked files
	activityFiles := s.activity.RecentFiles(limit)

	// Build the merged result
	seen := make(map[fileKey]bool)
	var withActivity []RecentFile
	var withoutActivity []RecentFile

	// First pass: activity-tracked files (these go first, sorted by activity time)
	for _, af := range activityFiles {
		key := fileKey{af.Project, af.FilePath}
		seen[key] = true

		rf := RecentFile{
			Project:      af.Project,
			FilePath:     af.FilePath,
			FileName:     af.FileName,
			DisplayName:  af.FileName,
			ActivityType: string(af.Type),
			ActivityAge:  formatAge(af.Timestamp),
		}

		// Fill in modtime/fileType from cache if available
		if cf, ok := cachedByKey[key]; ok {
			rf.ModTime = cf.ModTime
			rf.Age = formatAge(cf.ModTime)
			rf.FileType = cf.FileType
			rf.DisplayName = cf.Name
		} else {
			rf.ModTime = af.Timestamp
			rf.Age = formatAge(af.Timestamp)
		}

		withActivity = append(withActivity, rf)
	}

	// Second pass: cached files without tracked activity
	for _, f := range cachedFiles {
		key := fileKey{f.Project, f.FullPath}
		if seen[key] {
			continue
		}

		// Check if there's an activity entry not in the top N
		act := s.activity.Lookup(f.Project, f.FullPath)

		rf := RecentFile{
			Project:     f.Project,
			FilePath:    f.FullPath,
			FileName:    f.Name,
			DisplayName: f.Name,
			ModTime:     f.ModTime,
			Age:         formatAge(f.ModTime),
			FileType:    f.FileType,
		}

		if act != nil {
			rf.ActivityType = string(act.Type)
			rf.ActivityAge = formatAge(act.Timestamp)
			withActivity = append(withActivity, rf)
		} else {
			withoutActivity = append(withoutActivity, rf)
		}
	}

	// Sort activity files by activity timestamp
	sort.SliceStable(withActivity, func(i, j int) bool {
		ai := s.activity.Lookup(withActivity[i].Project, withActivity[i].FilePath)
		aj := s.activity.Lookup(withActivity[j].Project, withActivity[j].FilePath)
		if ai == nil || aj == nil {
			return ai != nil
		}
		return ai.Timestamp.After(aj.Timestamp)
	})

	result := append(withActivity, withoutActivity...)
	if limit > 0 && len(result) > limit {
		result = result[:limit]
	}
	return result
}
