package publish

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// PublishState stores metadata about a published file.
type PublishState struct {
	SiteName      string    `json:"siteName"`
	URL           string    `json:"url"`
	LastPublished time.Time `json:"lastPublished"`
}

var stateMu sync.Mutex

// LoadState reads the publish state for a project. Returns map of filePath -> state.
// E-PENPAL-PUBLISH-STATE: reads publish.json state map with Mutex.
func LoadState(projectPath string) (map[string]*PublishState, error) {
	stateMu.Lock()
	defer stateMu.Unlock()

	path := stateFilePath(projectPath)
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return make(map[string]*PublishState), nil
		}
		return nil, err
	}
	var state map[string]*PublishState
	if err := json.Unmarshal(data, &state); err != nil {
		return nil, err
	}
	if state == nil {
		state = make(map[string]*PublishState)
	}
	return state, nil
}

// SaveState writes a single file's publish state.
// E-PENPAL-PUBLISH-STATE: writes publish.json state map with Mutex.
func SaveState(projectPath, filePath string, state *PublishState) error {
	stateMu.Lock()
	defer stateMu.Unlock()

	path := stateFilePath(projectPath)

	// Load existing state
	existing := make(map[string]*PublishState)
	if data, err := os.ReadFile(path); err == nil {
		json.Unmarshal(data, &existing)
	}
	if existing == nil {
		existing = make(map[string]*PublishState)
	}

	existing[filePath] = state

	data, err := json.MarshalIndent(existing, "", "  ")
	if err != nil {
		return err
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}
	return os.WriteFile(path, data, 0644)
}

func stateFilePath(projectPath string) string {
	return filepath.Join(projectPath, ".penpal", "publish.json")
}
