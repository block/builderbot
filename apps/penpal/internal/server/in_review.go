package server

import (
	"encoding/json"
	"net/http"
	"path/filepath"

	"github.com/loganj/penpal/internal/discovery"
)

// ReviewFileEntry is a single file within a review group.
type ReviewFileEntry struct {
	Name           string `json:"name"`
	Title          string `json:"title,omitempty"`
	Path           string `json:"path"`
	Project        string `json:"project"`
	ProjectPath    string `json:"projectPath"`
	OpenThreads    int    `json:"openThreads"`
	AgentActive    bool   `json:"agentActive"`
	WorkingThreads int    `json:"workingThreads,omitempty"`
	FileType       string `json:"fileType,omitempty"`
	Age            string `json:"age,omitempty"`
	Source         string `json:"source,omitempty"`
	SourceType     string `json:"sourceType,omitempty"`
}

// ReviewGroup groups files in review by project and source, with breadcrumb info.
type ReviewGroup struct {
	Workspace      string            `json:"workspace,omitempty"`
	WorkspaceURL   string            `json:"workspaceURL,omitempty"`
	ProjectName    string            `json:"projectName"`
	ProjectQN      string            `json:"projectQN"`
	SourceName     string            `json:"sourceName"`
	SourceAnchor   string            `json:"sourceAnchor"`
	BadgeText      string            `json:"badgeText,omitempty"`
	BadgeColor     string            `json:"badgeColor,omitempty"`
	BadgeBg        string            `json:"badgeBg,omitempty"`
	AgentActive    bool              `json:"agentActive"`
	WorkingThreads int               `json:"workingThreads,omitempty"`
	Files          []ReviewFileEntry `json:"files"`
}

// E-PENPAL-IN-REVIEW-PAGE: GET /api/in-review groups files with open threads.
func (s *Server) handleAPIInReview(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(s.listAllReviewGroups())
}

// listAllReviewGroups returns files in review grouped by project+source.
// E-PENPAL-IN-REVIEW-PAGE: groups files with open threads by project and source.
func (s *Server) listAllReviewGroups() []ReviewGroup {
	projects := s.cache.Projects()

	// Build source type lookup per project+source
	type groupKey struct {
		project string
		source  string
	}
	groupMap := make(map[groupKey]*ReviewGroup)
	var groupOrder []groupKey

	for _, p := range projects {
		qn := p.QualifiedName()
		reviews, err := s.comments.ListFilesInReview(qn)
		if err != nil || len(reviews) == 0 {
			continue
		}

		// E-PENPAL-AGENT-ACTIVE-UNIFIED: checks both spawned agents and CLI sessions.
		agentActive := s.isAgentActive(qn)

		// Build source name -> SourceTypeName lookup
		sourceTypeMap := make(map[string]string)
		for _, src := range p.Sources {
			sourceTypeMap[src.Name] = src.SourceTypeName
		}

		for _, f := range reviews {
			workingThreads := s.comments.WorkingCount(qn, f.FilePath)
			if agentActive && workingThreads == 0 {
				workingThreads = s.comments.WorkingCountNoExpiry(qn, f.FilePath)
			}

			// Look up source from cache
			sourceName := ""
			fileType := ""
			age := ""
			sourceType := ""
			title := ""
			if fi := s.cache.FindFile(qn, f.FilePath); fi != nil {
				sourceName = fi.Source
				fileType = fi.FileType
				age = formatAge(fi.ModTime)
				sourceType = fi.SourceType
				title = fi.Title
			}
			if sourceName == "" {
				sourceName = "files"
			}

			key := groupKey{project: qn, source: sourceName}
			g, ok := groupMap[key]
			if !ok {
				// Look up badge info
				var badgeText, badgeColor, badgeBg string
				if stName, ok := sourceTypeMap[sourceName]; ok {
					if st := discovery.GetSourceType(stName); st != nil {
						badgeText = st.DisplayName
						badgeColor = st.BadgeColor
						badgeBg = st.BadgeBg
					}
				}

				g = &ReviewGroup{
					Workspace:    p.WorkspaceName,
					ProjectName:  p.Name,
					ProjectQN:    qn,
					SourceName:   sourceName,
					SourceAnchor: "source-" + sourceName,
					BadgeText:    badgeText,
					BadgeColor:   badgeColor,
					BadgeBg:      badgeBg,
					AgentActive:  agentActive,
					Files:        make([]ReviewFileEntry, 0),
				}
				groupMap[key] = g
				groupOrder = append(groupOrder, key)
			}

			name := filepath.Base(f.FilePath)

			g.Files = append(g.Files, ReviewFileEntry{
				Name:           name,
				Title:          title,
				Path:           f.FilePath,
				Project:        qn,
				ProjectPath:    p.Path,
				OpenThreads:    f.OpenThreads,
				AgentActive:    agentActive,
				WorkingThreads: workingThreads,
				FileType:       fileType,
				Age:            age,
				Source:         sourceName,
				SourceType:     sourceType,
			})
			g.WorkingThreads += workingThreads
		}
	}

	result := make([]ReviewGroup, 0, len(groupOrder))
	for _, key := range groupOrder {
		result = append(result, *groupMap[key])
	}
	return result
}
