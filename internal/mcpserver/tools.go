package mcpserver

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// --- Input types for each tool ---

type listThreadsInput struct {
	Project string `json:"project" jsonschema:"Project name"`
	Path    string `json:"path,omitempty" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	Status  string `json:"status,omitempty" jsonschema:"Filter by status: open or resolved"`
}

type readThreadInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	Path     string `json:"path" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	ThreadID string `json:"threadId" jsonschema:"Thread ID"`
}

type replyInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	Path     string `json:"path" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	ThreadID string `json:"threadId" jsonschema:"Thread ID to reply to"`
	Body     string `json:"body" jsonschema:"Reply message body"`
}

type createThreadInput struct {
	Project      string `json:"project" jsonschema:"Project name"`
	Path         string `json:"path" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	SelectedText string `json:"selectedText" jsonschema:"The text in the file to anchor the comment to"`
	Body         string `json:"body" jsonschema:"Comment body"`
	HeadingPath  string `json:"headingPath,omitempty" jsonschema:"Heading path for context"`
}

type filesInReviewInput struct {
	Project string `json:"project" jsonschema:"Project name"`
}

type waitForChangesInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	SinceSeq uint64 `json:"sinceSeq,omitempty" jsonschema:"Sequence number from previous wait call. Changes since this seq return immediately."`
}

type findProjectInput struct {
	Directory string `json:"directory" jsonschema:"Absolute path to a directory inside the project (typically your working directory)"`
}

// textResult returns a CallToolResult containing a single JSON text block.
func textResult(v any) (*mcp.CallToolResult, error) {
	data, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("marshaling result: %w", err)
	}
	return &mcp.CallToolResult{
		Content: []mcp.Content{&mcp.TextContent{Text: string(data)}},
	}, nil
}

// registerTools adds all birdseye MCP tools to the server.
func registerTools(server *mcp.Server, store *comments.Store, c *cache.Cache) {
	// birdseye_list_threads
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_list_threads",
		Description: "List comment threads on documentation files. Paths are relative to the project root (e.g., thoughts/plans/foo.md). When path is omitted, returns all open threads across the project. Optionally filter by status (open/resolved).",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input listThreadsInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" {
			return nil, nil, fmt.Errorf("project is required")
		}

		if input.Path == "" {
			// List open threads across the entire project
			store.RecordHeartbeat(input.Project, "")
			threads, err := store.ListOpenThreads(input.Project)
			if err != nil {
				return nil, nil, err
			}
			// Record heartbeat for each file
			seen := make(map[string]bool)
			for _, t := range threads {
				if !seen[t.FilePath] {
					store.RecordHeartbeat(input.Project, t.FilePath)
					seen[t.FilePath] = true
				}
			}
			res, err := textResult(threads)
			return res, nil, err
		}

		// Load threads for a specific file
		store.RecordHeartbeat(input.Project, input.Path)
		fc, err := store.Load(input.Project, input.Path)
		if err != nil {
			return nil, nil, err
		}

		var filtered []comments.Thread
		for _, t := range fc.Threads {
			if input.Status == "" || t.Status == input.Status {
				filtered = append(filtered, t)
			}
		}
		res, err := textResult(filtered)
		return res, nil, err
	})

	// birdseye_read_thread
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_read_thread",
		Description: "Read a full comment thread on a document. Path is relative to project root (e.g., thoughts/plans/foo.md). Returns the complete thread JSON with all comments.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input readThreadInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" || input.Path == "" || input.ThreadID == "" {
			return nil, nil, fmt.Errorf("project, path, and threadId are all required")
		}

		fc, err := store.Load(input.Project, input.Path)
		if err != nil {
			return nil, nil, err
		}

		for _, t := range fc.Threads {
			if t.ID == input.ThreadID {
				res, err := textResult(t)
				return res, nil, err
			}
		}
		return nil, nil, fmt.Errorf("thread not found: %s", input.ThreadID)
	})

	// birdseye_reply
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_reply",
		Description: "Reply to an existing comment thread. The reply is attributed to the agent.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input replyInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" || input.Path == "" || input.ThreadID == "" || input.Body == "" {
			return nil, nil, fmt.Errorf("project, path, threadId, and body are all required")
		}

		comment := comments.Comment{
			Author: "claude",
			Role:   "agent",
			Body:   input.Body,
		}
		thread, err := store.AddComment(input.Project, input.Path, input.ThreadID, comment)
		if err != nil {
			return nil, nil, err
		}
		res, err := textResult(thread)
		return res, nil, err
	})

	// birdseye_create_thread
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_create_thread",
		Description: "Create a new comment thread anchored to specific text in a markdown document. Path is relative to project root (e.g., thoughts/plans/foo.md). The before/after context is computed automatically by finding the selectedText in the file.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input createThreadInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" || input.Path == "" || input.SelectedText == "" || input.Body == "" {
			return nil, nil, fmt.Errorf("project, path, selectedText, and body are all required")
		}

		// Read the markdown file to compute anchor context
		project := c.FindProject(input.Project)
		if project == nil {
			return nil, nil, fmt.Errorf("project not found: %s", input.Project)
		}

		fullPath := filepath.Join(project.Path, input.Path)
		content, err := os.ReadFile(fullPath)
		if err != nil {
			return nil, nil, fmt.Errorf("reading file: %w", err)
		}
		markdown := string(content)

		// Find the selected text and compute context
		anchor := comments.Anchor{
			SelectedText: input.SelectedText,
			HeadingPath:  input.HeadingPath,
		}

		idx := strings.Index(markdown, input.SelectedText)
		if idx >= 0 {
			// Extract ~80 chars before
			beforeStart := idx - 80
			if beforeStart < 0 {
				beforeStart = 0
			}
			anchor.Before = markdown[beforeStart:idx]

			// Extract ~80 chars after
			afterStart := idx + len(input.SelectedText)
			afterEnd := afterStart + 80
			if afterEnd > len(markdown) {
				afterEnd = len(markdown)
			}
			anchor.After = markdown[afterStart:afterEnd]
		}

		comment := comments.Comment{
			Author: "claude",
			Role:   "agent",
			Body:   input.Body,
		}

		thread, err := store.CreateThread(input.Project, input.Path, anchor, comment)
		if err != nil {
			return nil, nil, err
		}
		res, err := textResult(thread)
		return res, nil, err
	})

	// birdseye_files_in_review
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_files_in_review",
		Description: "List all documentation files currently in review for a project. File paths are relative to the project root (e.g., thoughts/plans/foo.md). Records a heartbeat for each file to signal agent presence in the birdseye UI.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input filesInReviewInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" {
			return nil, nil, fmt.Errorf("project is required")
		}

		files, err := store.ListFilesInReview(input.Project)
		if err != nil {
			return nil, nil, err
		}

		// Record heartbeat for each file in review
		for _, f := range files {
			store.RecordHeartbeat(input.Project, f.FilePath)
		}

		res, err := textResult(files)
		return res, nil, err
	})

	// birdseye_wait_for_changes
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_wait_for_changes",
		Description: "Block until comment threads change for a project (new thread, reply, resolve, or reopen), or until timeout (30s). Returns the current files in review. Use this in a loop instead of polling birdseye_files_in_review. Also records agent heartbeat. Pass the `seq` value from the previous response as `sinceSeq` to avoid missing changes between calls.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input waitForChangesInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" {
			return nil, nil, fmt.Errorf("project is required")
		}

		// Record heartbeat at start of wait
		store.RecordHeartbeat(input.Project, "")

		waitCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
		defer cancel()

		seq, waitErr := store.WaitForChangeSince(waitCtx, input.SinceSeq)
		changed := waitErr == nil

		// Record heartbeat after waking
		store.RecordHeartbeat(input.Project, "")

		files, err := store.ListFilesInReview(input.Project)
		if err != nil {
			return nil, nil, err
		}

		for _, f := range files {
			store.RecordHeartbeat(input.Project, f.FilePath)
		}

		result := map[string]any{
			"changed": changed,
			"seq":     seq,
			"files":   files,
		}
		res, err := textResult(result)
		return res, nil, err
	})

	// birdseye_find_project
	mcp.AddTool(server, &mcp.Tool{
		Name:        "birdseye_find_project",
		Description: "Find the birdseye project for a given directory. Returns the project name to use with other birdseye tools. Call this first if you don't already know your project name.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input findProjectInput) (*mcp.CallToolResult, any, error) {
		if input.Directory == "" {
			return nil, nil, fmt.Errorf("directory is required")
		}

		project := c.FindProjectByPath(input.Directory)
		if project == nil {
			return nil, nil, fmt.Errorf("no project found for directory: %s", input.Directory)
		}

		result := map[string]string{
			"project": project.QualifiedName(),
			"path":    project.Path,
		}
		res, err := textResult(result)
		return res, nil, err
	})
}
