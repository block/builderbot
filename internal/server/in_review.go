package server

import (
	"encoding/json"
	"net/http"
)

// ReviewFile represents a file with open comment threads, for the global In Review page.
type ReviewFile struct {
	Project       string `json:"project"`
	FilePath      string `json:"filePath"`
	FileName      string `json:"fileName"`
	OpenThreads   int    `json:"openThreads"`
	AgentActive   bool   `json:"agentActive"`
	TypingThreads int    `json:"typingThreads,omitempty"`
}

func (s *Server) handleInReview(w http.ResponseWriter, r *http.Request) {
	files := s.listAllReviews()

	nav := s.buildNav("")
	pageData := struct {
		Files []ReviewFile
	}{
		Files: files,
	}
	s.renderPage(w, "in-review.html", nav, pageData)
}

func (s *Server) handleAPIInReview(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(s.listAllReviews())
}

// listAllReviews returns all files with open threads across every project.
func (s *Server) listAllReviews() []ReviewFile {
	projects := s.cache.Projects()
	var result []ReviewFile

	for _, p := range projects {
		qn := p.QualifiedName()
		reviews, err := s.comments.ListFilesInReview(qn)
		if err != nil || len(reviews) == 0 {
			continue
		}

		agentActive := s.agents != nil && s.agents.Status(qn) != nil && s.agents.Status(qn).Running

		for _, f := range reviews {
			typingThreads := s.comments.TypingCount(qn, f.FilePath)
			if agentActive && typingThreads == 0 {
				typingThreads = s.comments.TypingCountNoExpiry(qn, f.FilePath)
			}

			// Extract filename from path
			name := f.FilePath
			for i := len(name) - 1; i >= 0; i-- {
				if name[i] == '/' {
					name = name[i+1:]
					break
				}
			}

			result = append(result, ReviewFile{
				Project:       qn,
				FilePath:      f.FilePath,
				FileName:      name,
				OpenThreads:   f.OpenThreads,
				AgentActive:   agentActive,
				TypingThreads: typingThreads,
			})
		}
	}

	return result
}
