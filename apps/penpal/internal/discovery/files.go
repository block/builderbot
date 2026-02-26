package discovery

import (
	"os"
	"path/filepath"
	"sort"
	"time"
)

type ThoughtFile struct {
	Name    string
	Path    string // relative to thoughts/
	ModTime time.Time
	IsDir   bool
}

func ListThoughts(thoughtsPath, subpath string) ([]ThoughtFile, error) {
	dir := filepath.Join(thoughtsPath, subpath)
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}

	var files []ThoughtFile
	for _, entry := range entries {
		if entry.Name()[0] == '.' {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		relPath := filepath.Join(subpath, entry.Name())
		files = append(files, ThoughtFile{
			Name:    entry.Name(),
			Path:    relPath,
			ModTime: info.ModTime(),
			IsDir:   entry.IsDir(),
		})
	}

	// Sort: directories first, then by mod time (newest first)
	sort.Slice(files, func(i, j int) bool {
		if files[i].IsDir != files[j].IsDir {
			return files[i].IsDir
		}
		return files[i].ModTime.After(files[j].ModTime)
	})

	return files, nil
}
