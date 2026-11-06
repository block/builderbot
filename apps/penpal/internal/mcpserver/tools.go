package mcpserver

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// --- Input types for each tool ---

type listThreadsInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	Path     string `json:"path,omitempty" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	Status   string `json:"status,omitempty" jsonschema:"Filter by status: open or resolved"`
	Worktree string `json:"worktree,omitempty" jsonschema:"Worktree name to scope comments to. Omit for main worktree."`
}

type readThreadInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	Path     string `json:"path" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	ThreadID string `json:"threadId" jsonschema:"Thread ID"`
	Worktree string `json:"worktree,omitempty" jsonschema:"Worktree name to scope comments to. Omit for main worktree."`
}

type replyInput struct {
	Project          string   `json:"project" jsonschema:"Project name"`
	Path             string   `json:"path" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	ThreadID         string   `json:"threadId" jsonschema:"Thread ID to reply to"`
	Body             string   `json:"body" jsonschema:"Reply message body"`
	SuggestedReplies []string `json:"suggestedReplies,omitempty" jsonschema:"Up to 3 short reply suggestions shown as clickable pills to the human"`
	Worktree         string   `json:"worktree,omitempty" jsonschema:"Worktree name to scope comments to. Omit for main worktree."`
}

type createThreadInput struct {
	Project          string   `json:"project" jsonschema:"Project name"`
	Path             string   `json:"path" jsonschema:"File path relative to project root, e.g. thoughts/plans/foo.md"`
	SelectedText     string   `json:"selectedText" jsonschema:"The text in the file to anchor the comment to"`
	Body             string   `json:"body" jsonschema:"Comment body"`
	HeadingPath      string   `json:"headingPath,omitempty" jsonschema:"Heading path for context"`
	SuggestedReplies []string `json:"suggestedReplies,omitempty" jsonschema:"Up to 3 short reply suggestions shown as clickable pills to the human"`
	Worktree         string   `json:"worktree,omitempty" jsonschema:"Worktree name to scope comments to. Omit for main worktree."`
}

type filesInReviewInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	Worktree string `json:"worktree,omitempty" jsonschema:"Worktree name to scope comments to. Omit for main worktree."`
}

type waitForChangesInput struct {
	Project  string `json:"project" jsonschema:"Project name"`
	SinceSeq uint64 `json:"sinceSeq,omitempty" jsonschema:"Sequence number from previous wait call. Changes since this seq return immediately."`
	Worktree string `json:"worktree,omitempty" jsonschema:"Worktree name to scope comments to. Omit for main worktree."`
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

// registerTools adds all penpal MCP tools to the server.
// E-PENPAL-MCP-TOOLS: registers penpal_find_project, penpal_list_threads, penpal_read_thread, penpal_reply, penpal_create_thread, penpal_files_in_review, penpal_wait_for_changes.
// E-PENPAL-AGENT-SELF-ID: agentNameFunc derives the comment author from the session.
func registerTools(server *mcp.Server, store *comments.Store, c *cache.Cache, agentNameFunc func(project string) string) {
	// penpal_list_threads
	// E-PENPAL-MCP-TOOLS: penpal_list_threads lists threads by file or project-wide.
	// E-PENPAL-MCP-WORKING: auto-sets working indicator for threads where last comment is from human.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_list_threads",
		Description: "List comment threads on documentation files. Paths are relative to the project root (e.g., thoughts/plans/foo.md). When path is omitted, returns all open threads across the project. Optionally filter by status (open/resolved).",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input listThreadsInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" {
			return nil, nil, fmt.Errorf("project is required")
		}

		if input.Path == "" {
			// List threads across the entire project, filtered by status
			status := input.Status
			if status == "" {
				status = "open"
			}
			threads, err := store.ListThreadsByStatusForWorktree(input.Project, status, input.Worktree)
			if err != nil {
				return nil, nil, err
			}
			store.MarkThreadsRead(input.Project, threads)
			res, err := textResult(threads)
			return res, nil, err
		}

		// Load threads for a specific file
		fc, err := store.LoadForWorktree(input.Project, input.Path, input.Worktree)
		if err != nil {
			return nil, nil, err
		}

		var filtered []comments.Thread
		for _, t := range fc.Threads {
			if input.Status == "" || t.Status == input.Status {
				filtered = append(filtered, t)
			}
		}
		store.MarkFileThreadsRead(input.Project, input.Path, fc.Threads)
		res, err := textResult(filtered)
		return res, nil, err
	})

	// penpal_read_thread
	// E-PENPAL-MCP-TOOLS: penpal_read_thread returns full thread with all comments.
	// E-PENPAL-MCP-WORKING: auto-sets working indicator when last comment is from human.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_read_thread",
		Description: "Read a full comment thread on a document. Path is relative to project root (e.g., thoughts/plans/foo.md). Returns the complete thread JSON with all comments.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input readThreadInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" || input.Path == "" || input.ThreadID == "" {
			return nil, nil, fmt.Errorf("project, path, and threadId are all required")
		}

		fc, err := store.LoadForWorktree(input.Project, input.Path, input.Worktree)
		if err != nil {
			return nil, nil, err
		}

		for _, t := range fc.Threads {
			if t.ID == input.ThreadID {
				store.MarkFileThreadsRead(input.Project, input.Path, []comments.Thread{t})
				res, err := textResult(t)
				return res, nil, err
			}
		}
		return nil, nil, fmt.Errorf("thread not found: %s", input.ThreadID)
	})

	// penpal_reply
	// E-PENPAL-MCP-TOOLS: penpal_reply adds agent reply and clears working indicator.
	// E-PENPAL-MCP-WORKING: clears working indicator on reply.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_reply",
		Description: "Reply to an existing comment thread. The reply is attributed to the agent. Include suggestedReplies when asking for confirmation or presenting options, but only for meaningful responses the human would type — not generic ones like \"yes\"/\"no\"/\"looks good\" that duplicate the reply/resolve buttons.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input replyInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" || input.Path == "" || input.ThreadID == "" || input.Body == "" {
			return nil, nil, fmt.Errorf("project, path, threadId, and body are all required")
		}

		// Working indicator handling (InReplyTo, WorkingStartedAt, ClearWorking)
		// is done automatically by AddCommentForWorktree for agent-role comments.
		// E-PENPAL-AGENT-SELF-ID: derive author from session via agentNameFunc.
		comment := comments.Comment{
			Author:           agentNameFunc(input.Project),
			Role:             "agent",
			Body:             input.Body,
			SuggestedReplies: input.SuggestedReplies,
		}
		thread, err := store.AddCommentForWorktree(input.Project, input.Path, input.Worktree, input.ThreadID, comment)
		if err != nil {
			return nil, nil, err
		}

		res, err := textResult(thread)
		return res, nil, err
	})

	// penpal_create_thread
	// E-PENPAL-MCP-TOOLS: penpal_create_thread computes Before/After/StartLine from disk and creates thread.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_create_thread",
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

		// Use worktree path if specified, otherwise project path
		basePath := project.Path
		if input.Worktree != "" {
			wtPath := c.WorktreePath(input.Project, input.Worktree)
			if wtPath == "" {
				return nil, nil, fmt.Errorf("worktree not found: %s", input.Worktree)
			}
			basePath = wtPath
		}

		fullPath := filepath.Join(basePath, input.Path)
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

			// Store line number for fallback anchoring
			line := 1
			for i := 0; i < idx; i++ {
				if markdown[i] == '\n' {
					line++
				}
			}
			anchor.StartLine = line
		}

		// E-PENPAL-AGENT-SELF-ID: derive author from session via agentNameFunc.
		comment := comments.Comment{
			Author:           agentNameFunc(input.Project),
			Role:             "agent",
			Body:             input.Body,
			SuggestedReplies: input.SuggestedReplies,
		}

		thread, err := store.CreateThreadForWorktree(input.Project, input.Path, input.Worktree, anchor, comment)
		if err != nil {
			return nil, nil, err
		}
		res, err := textResult(thread)
		return res, nil, err
	})

	// penpal_files_in_review
	// E-PENPAL-MCP-TOOLS: penpal_files_in_review lists files with open threads, enriched with oldest pending.
	// E-PENPAL-MCP-WORKING: auto-sets working indicator for oldest pending thread.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_files_in_review",
		Description: "List all documentation files currently in review for a project. File paths are relative to the project root (e.g., thoughts/plans/foo.md). Records a heartbeat for each file to signal agent presence in the penpal UI. For each file, includes all open threads and the full content of the oldest pending thread (where the last comment is from a human). The working indicator is set for the oldest pending thread so the UI shows the agent is working on it.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input filesInReviewInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" {
			return nil, nil, fmt.Errorf("project is required")
		}

		files, err := store.ListFilesInReviewForWorktree(input.Project, input.Worktree)
		if err != nil {
			return nil, nil, err
		}

		type fileWithThreads struct {
			FilePath      string            `json:"filePath"`
			OpenThreads   int               `json:"openThreads"`
			Threads       []comments.Thread `json:"threads,omitempty"`
			OldestPending *comments.Thread  `json:"oldestPending,omitempty"`
		}

		enrichedFiles := make([]fileWithThreads, 0, len(files))
		for _, f := range files {
			ef := fileWithThreads{
				FilePath:    f.FilePath,
				OpenThreads: f.OpenThreads,
			}

			fc, loadErr := store.LoadForWorktree(input.Project, f.FilePath, input.Worktree)
			if loadErr == nil {
				var oldestPending *comments.Thread
				var oldestTime time.Time

				for _, t := range fc.Threads {
					if t.Status == "open" {
						ef.Threads = append(ef.Threads, t)
						if len(t.Comments) > 0 && t.Comments[len(t.Comments)-1].Role == "human" {
							if oldestPending == nil || t.Comments[len(t.Comments)-1].CreatedAt.Before(oldestTime) {
								tCopy := t
								oldestPending = &tCopy
								oldestTime = t.Comments[len(t.Comments)-1].CreatedAt
							}
						}
					}
				}

				if oldestPending != nil {
					ef.OldestPending = oldestPending
				}
				store.MarkFileThreadsRead(input.Project, f.FilePath, fc.Threads)
			}

			enrichedFiles = append(enrichedFiles, ef)
		}

		res, err := textResult(enrichedFiles)
		return res, nil, err
	})

	// penpal_wait_for_changes
	// E-PENPAL-MCP-TOOLS: penpal_wait_for_changes blocks via 30s long-poll for comment changes.
	// E-PENPAL-CHANGE-SEQ: uses WaitAndEnrich to block, enrich, and refresh working timestamps.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_wait_for_changes",
		Description: "Block until comment threads change for a project (new thread, reply, resolve, or reopen), or until timeout (30s). Returns the current files in review. Use this in a loop instead of polling penpal_files_in_review. Also records agent heartbeat. Pass the `seq` value from the previous response as `sinceSeq` to avoid missing changes between calls.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input waitForChangesInput) (*mcp.CallToolResult, any, error) {
		if input.Project == "" {
			return nil, nil, fmt.Errorf("project is required")
		}

		waitCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
		defer cancel()

		result, err := store.WaitAndEnrich(waitCtx, input.Project, input.Worktree, input.SinceSeq)
		if err != nil {
			return nil, nil, err
		}

		res, err := textResult(result)
		return res, nil, err
	})

	// penpal_find_project
	// E-PENPAL-MCP-TOOLS: penpal_find_project maps CWD to project name and optional worktree.
	mcp.AddTool(server, &mcp.Tool{
		Name:        "penpal_find_project",
		Description: "Find the penpal project for a given directory. Returns the project name and optional worktree to use with other penpal tools. Call this first if you don't already know your project name.",
	}, func(ctx context.Context, req *mcp.CallToolRequest, input findProjectInput) (*mcp.CallToolResult, any, error) {
		if input.Directory == "" {
			return nil, nil, fmt.Errorf("directory is required")
		}

		project, worktree := c.FindProjectByPathWithWorktree(input.Directory)
		if project == nil {
			return nil, nil, fmt.Errorf("no project found for directory: %s", input.Directory)
		}

		result := map[string]string{
			"project": project.QualifiedName(),
			"path":    project.Path,
		}
		if worktree != "" {
			result["worktree"] = worktree
		}
		res, err := textResult(result)
		return res, nil, err
	})
}
