package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/loganj/penpal/internal/config"
)

func main() {
	args := os.Args[1:]
	if len(args) == 0 {
		printUsage()
		os.Exit(1)
	}

	switch args[0] {
	case "open":
		if len(args) < 2 {
			fmt.Fprintf(os.Stderr, "Usage: penpal open <path>...\n")
			os.Exit(1)
		}
		runOpen(args[1:])
	case "attach":
		runAttach(args[1:])
	case "files-in-review":
		runFilesInReview(args[1:])
	case "list-threads":
		runListThreads(args[1:])
	case "read-thread":
		runReadThread(args[1:])
	case "reply":
		runReply(args[1:])
	case "create-thread":
		runCreateThread(args[1:])
	case "wait":
		runWait(args[1:])
	default:
		fmt.Fprintf(os.Stderr, "Unknown command: %s\n", args[0])
		printUsage()
		os.Exit(1)
	}
}

func printUsage() {
	fmt.Fprintf(os.Stderr, `Usage: penpal <command> [options]

Commands:
  open <path>...                         Open files/directories in Penpal
  attach <path> [--force] [--agent NAME]  Attach as the active agent for a project
  files-in-review --project P [--worktree W]
  list-threads --project P [--path F] [--status S] [--worktree W]
  read-thread --project P --path F --thread-id ID [--worktree W]
  reply --session T --project P --path F --thread-id ID [--body B] [--worktree W]
  create-thread --session T --project P --path F --selected-text T --body B [--heading-path H] [--worktree W]
  wait --session T --project P [--since-seq N] [--worktree W]
`)
}

// --- open command ---

// runOpen opens paths in the Penpal desktop app, launching it if needed.
// E-PENPAL-CLI: reads port file, checks health, calls POST /api/open.
func runOpen(paths []string) {
	port := ensureServer()
	openPaths(port, paths)
}

// --- attach command ---

// runAttach registers the calling agent as the active agent for a project.
// E-PENPAL-CLI-ATTACH: resolves path, ensures server, POST /api/agents/attach.
func runAttach(args []string) {
	fs := flag.NewFlagSet("attach", flag.ExitOnError)
	force := fs.Bool("force", false, "Evict existing agent and take over")
	agent := fs.String("agent", "", "Agent name (e.g., amp, claude)")
	fs.Parse(args)

	if fs.NArg() < 1 {
		fmt.Fprintf(os.Stderr, "Usage: penpal attach <path> [--force] [--agent NAME]\n")
		os.Exit(1)
	}

	absPath, err := filepath.Abs(fs.Arg(0))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not resolve path: %v\n", err)
		os.Exit(1)
	}

	port := ensureServer()

	attachBody := map[string]any{
		"path":  absPath,
		"force": *force,
	}
	if *agent != "" {
		attachBody["agent"] = *agent
	}
	body, _ := json.Marshal(attachBody)

	resp, err := http.Post(
		fmt.Sprintf("http://localhost:%d/api/agents/attach", port),
		"application/json",
		bytes.NewReader(body),
	)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not contact server: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusConflict {
		var errResp struct {
			Error string `json:"error"`
		}
		json.NewDecoder(resp.Body).Decode(&errResp)
		fmt.Fprintf(os.Stderr, "Error: %s\n", errResp.Error)
		os.Exit(1)
	}

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		fmt.Fprintf(os.Stderr, "Error: server returned %d: %s\n", resp.StatusCode, strings.TrimSpace(string(bodyBytes)))
		os.Exit(1)
	}

	io.Copy(os.Stdout, resp.Body)
	fmt.Fprintln(os.Stdout)
}

// --- files-in-review command ---

// runFilesInReview queries files with open threads for a project.
// E-PENPAL-CLI-AGENT-CMDS: GET /api/reviews — read-only, no session required.
func runFilesInReview(args []string) {
	fs := flag.NewFlagSet("files-in-review", flag.ExitOnError)
	session := fs.String("session", "", "Session token (optional, records heartbeat)")
	project := fs.String("project", "", "Project qualified name (required)")
	worktree := fs.String("worktree", "", "Worktree name")
	fs.Parse(args)

	if *project == "" {
		fmt.Fprintf(os.Stderr, "Error: --project is required for files-in-review\n")
		os.Exit(1)
	}

	port := getPort()
	u := fmt.Sprintf("http://localhost:%d/api/reviews?project=%s",
		port, urlEncode(*project))
	if *session != "" {
		u += "&session=" + urlEncode(*session)
	}
	if *worktree != "" {
		u += "&worktree=" + urlEncode(*worktree)
	}

	doGet(u)
}

// --- list-threads command ---

// runListThreads lists comment threads for a project or file.
// E-PENPAL-CLI-AGENT-CMDS: GET /api/threads — read-only, no session required.
func runListThreads(args []string) {
	fs := flag.NewFlagSet("list-threads", flag.ExitOnError)
	session := fs.String("session", "", "Session token (optional, records heartbeat)")
	project := fs.String("project", "", "Project qualified name (required)")
	path := fs.String("path", "", "File path (project-relative)")
	status := fs.String("status", "", "Filter by status (open, resolved)")
	worktree := fs.String("worktree", "", "Worktree name")
	fs.Parse(args)

	if *project == "" {
		fmt.Fprintf(os.Stderr, "Error: --project is required for list-threads\n")
		os.Exit(1)
	}

	port := getPort()
	u := fmt.Sprintf("http://localhost:%d/api/threads?project=%s",
		port, urlEncode(*project))
	if *session != "" {
		u += "&session=" + urlEncode(*session)
	}
	if *path != "" {
		u += "&path=" + urlEncode(*path)
	}
	if *status != "" {
		u += "&status=" + urlEncode(*status)
	}
	if *worktree != "" {
		u += "&worktree=" + urlEncode(*worktree)
	}

	doGet(u)
}

// --- read-thread command ---

// runReadThread reads a single thread by ID (filters from list-threads response).
// E-PENPAL-CLI-AGENT-CMDS: GET /api/threads — read-only, no session required.
func runReadThread(args []string) {
	fs := flag.NewFlagSet("read-thread", flag.ExitOnError)
	session := fs.String("session", "", "Session token (optional, records heartbeat)")
	project := fs.String("project", "", "Project qualified name (required)")
	path := fs.String("path", "", "File path, project-relative (required)")
	threadID := fs.String("thread-id", "", "Thread ID (required)")
	worktree := fs.String("worktree", "", "Worktree name")
	fs.Parse(args)

	if *project == "" || *path == "" || *threadID == "" {
		fmt.Fprintf(os.Stderr, "Usage: penpal read-thread --project P --path F --thread-id ID [--worktree W]\n")
		os.Exit(1)
	}

	port := getPort()
	u := fmt.Sprintf("http://localhost:%d/api/threads?project=%s&path=%s",
		port, urlEncode(*project), urlEncode(*path))
	if *session != "" {
		u += "&session=" + urlEncode(*session)
	}
	if *worktree != "" {
		u += "&worktree=" + urlEncode(*worktree)
	}

	resp, err := http.Get(u)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not contact server: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		fmt.Fprintf(os.Stderr, "Error: server returned %d: %s\n", resp.StatusCode, strings.TrimSpace(string(bodyBytes)))
		os.Exit(1)
	}

	var threads []json.RawMessage
	if err := json.NewDecoder(resp.Body).Decode(&threads); err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not decode response: %v\n", err)
		os.Exit(1)
	}

	for _, raw := range threads {
		var t struct {
			ID string `json:"id"`
		}
		if err := json.Unmarshal(raw, &t); err != nil {
			continue
		}
		if t.ID == *threadID {
			os.Stdout.Write(raw)
			fmt.Fprintln(os.Stdout)
			return
		}
	}

	fmt.Fprintf(os.Stderr, "Error: thread %s not found\n", *threadID)
	os.Exit(1)
}

// --- reply command ---

// runReply posts a reply to an existing thread.
// E-PENPAL-CLI-AGENT-CMDS: POST /api/threads/{id}/comments with session validation.
func runReply(args []string) {
	fs := flag.NewFlagSet("reply", flag.ExitOnError)
	session := fs.String("session", "", "Session token (required)")
	project := fs.String("project", "", "Project qualified name (required)")
	path := fs.String("path", "", "File path, project-relative (required)")
	threadID := fs.String("thread-id", "", "Thread ID (required)")
	body := fs.String("body", "", "Reply body (reads stdin if not provided)")
	worktree := fs.String("worktree", "", "Worktree name")
	fs.Parse(args)

	requireFlags(fs.Name(), *session, *project)
	if *path == "" || *threadID == "" {
		fmt.Fprintf(os.Stderr, "Usage: penpal reply --session T --project P --path F --thread-id ID [--body B] [--worktree W]\n")
		os.Exit(1)
	}

	replyBody := *body
	if replyBody == "" {
		data, err := io.ReadAll(os.Stdin)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error: could not read from stdin: %v\n", err)
			os.Exit(1)
		}
		replyBody = strings.TrimSpace(string(data))
	}
	if replyBody == "" {
		fmt.Fprintf(os.Stderr, "Error: reply body is required (--body or stdin)\n")
		os.Exit(1)
	}

	port := getPort()
	u := fmt.Sprintf("http://localhost:%d/api/threads/%s/comments?session=%s",
		port, urlEncode(*threadID), urlEncode(*session))

	payload, _ := json.Marshal(map[string]string{
		"project":  *project,
		"path":     *path,
		"author":   "claude",
		"role":     "agent",
		"body":     replyBody,
		"worktree": *worktree,
	})

	resp, err := http.Post(u, "application/json", bytes.NewReader(payload))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not contact server: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		fmt.Fprintf(os.Stderr, "Error: server returned %d: %s\n", resp.StatusCode, strings.TrimSpace(string(bodyBytes)))
		os.Exit(1)
	}

	io.Copy(os.Stdout, resp.Body)
	fmt.Fprintln(os.Stdout)
}

// --- create-thread command ---

// runCreateThread creates a new comment thread on a file.
// E-PENPAL-CLI-AGENT-CMDS: POST /api/threads with session validation and anchor.
func runCreateThread(args []string) {
	fs := flag.NewFlagSet("create-thread", flag.ExitOnError)
	session := fs.String("session", "", "Session token (required)")
	project := fs.String("project", "", "Project qualified name (required)")
	path := fs.String("path", "", "File path, project-relative (required)")
	selectedText := fs.String("selected-text", "", "Anchor text (required)")
	body := fs.String("body", "", "Comment body (required)")
	headingPath := fs.String("heading-path", "", "Heading path for anchor context")
	worktree := fs.String("worktree", "", "Worktree name")
	fs.Parse(args)

	requireFlags(fs.Name(), *session, *project)
	if *path == "" || *selectedText == "" || *body == "" {
		fmt.Fprintf(os.Stderr, "Usage: penpal create-thread --session T --project P --path F --selected-text T --body B [--heading-path H] [--worktree W]\n")
		os.Exit(1)
	}

	port := getPort()
	u := fmt.Sprintf("http://localhost:%d/api/threads?session=%s",
		port, urlEncode(*session))

	anchor := map[string]string{
		"selectedText": *selectedText,
	}
	if *headingPath != "" {
		anchor["headingPath"] = *headingPath
	}

	payload, _ := json.Marshal(map[string]any{
		"project":  *project,
		"path":     *path,
		"anchor":   anchor,
		"author":   "claude",
		"role":     "agent",
		"body":     *body,
		"worktree": *worktree,
	})

	resp, err := http.Post(u, "application/json", bytes.NewReader(payload))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not contact server: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		fmt.Fprintf(os.Stderr, "Error: server returned %d: %s\n", resp.StatusCode, strings.TrimSpace(string(bodyBytes)))
		os.Exit(1)
	}

	io.Copy(os.Stdout, resp.Body)
	fmt.Fprintln(os.Stdout)
}

// --- wait command ---

// runWait blocks until comments change or timeout (30s long-poll).
// E-PENPAL-CLI-AGENT-CMDS: GET /api/agents/wait with 35s client timeout.
func runWait(args []string) {
	fs := flag.NewFlagSet("wait", flag.ExitOnError)
	session := fs.String("session", "", "Session token (required)")
	project := fs.String("project", "", "Project qualified name (required)")
	sinceSeq := fs.String("since-seq", "", "Sequence number to wait after")
	worktree := fs.String("worktree", "", "Worktree name")
	fs.Parse(args)

	requireFlags(fs.Name(), *session, *project)

	port := getPort()
	u := fmt.Sprintf("http://localhost:%d/api/agents/wait?project=%s&session=%s",
		port, urlEncode(*project), urlEncode(*session))
	if *sinceSeq != "" {
		u += "&sinceSeq=" + urlEncode(*sinceSeq)
	}
	if *worktree != "" {
		u += "&worktree=" + urlEncode(*worktree)
	}

	client := &http.Client{Timeout: 35 * time.Second}
	resp, err := client.Get(u)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not contact server: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		fmt.Fprintf(os.Stderr, "Error: server returned %d: %s\n", resp.StatusCode, strings.TrimSpace(string(bodyBytes)))
		os.Exit(1)
	}

	io.Copy(os.Stdout, resp.Body)
	fmt.Fprintln(os.Stdout)
}

// --- shared helpers ---

// ensureServer checks for a running server, launching the app if needed.
// Returns the port of the running server.
// E-PENPAL-CLI: shared server startup logic for open and attach.
func ensureServer() int {
	port := config.ReadPortFile()

	if port > 0 && isServerRunning(port) {
		return port
	}

	openApp()

	port = waitForServerStart(10 * time.Second)
	if port <= 0 {
		fmt.Fprintf(os.Stderr, "Error: server did not start within timeout\n")
		os.Exit(1)
	}

	return port
}

// getPort reads the port file and returns the port.
// Exits if no server is running.
// E-PENPAL-CLI-AGENT-CMDS: used by agent commands that require a running server.
func getPort() int {
	port := config.ReadPortFile()
	if port <= 0 || !isServerRunning(port) {
		fmt.Fprintf(os.Stderr, "Error: penpal server is not running\n")
		os.Exit(1)
	}
	return port
}

// openPaths sends each path to the /api/open endpoint, then opens the desktop app.
// E-PENPAL-CLI: sends POST /api/open for each path.
func openPaths(port int, paths []string) {
	for _, arg := range paths {
		absPath, err := filepath.Abs(arg)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Warning: could not resolve path %q: %v\n", arg, err)
			continue
		}

		body, _ := json.Marshal(map[string]string{"path": absPath})
		resp, err := http.Post(
			fmt.Sprintf("http://localhost:%d/api/open", port),
			"application/json",
			bytes.NewReader(body),
		)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Warning: could not contact server for %q: %v\n", arg, err)
			continue
		}

		if resp.StatusCode != http.StatusOK {
			var errResp struct {
				Error string `json:"error"`
			}
			json.NewDecoder(resp.Body).Decode(&errResp)
			resp.Body.Close()
			fmt.Fprintf(os.Stderr, "Warning: server error for %q: %s\n", arg, errResp.Error)
			continue
		}
		resp.Body.Close()
	}

	// Open/focus the desktop app (deep link navigation is a future enhancement)
	openApp()
}

// isServerRunning checks if a penpal server is responding at the given port.
// E-PENPAL-CLI: health check against running penpal server.
func isServerRunning(port int) bool {
	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Get(fmt.Sprintf("http://localhost:%d/api/projects", port))
	if err != nil {
		return false
	}
	resp.Body.Close()
	return resp.StatusCode == http.StatusOK
}

// waitForServerStart polls for a newly launched server by re-reading the port
// file each iteration. This avoids polling a stale port left behind by a prior
// crash or dev run. Returns the port the server is running on, or 0 on timeout.
func waitForServerStart(timeout time.Duration) int {
	const defaultPort = 8080
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		port := config.ReadPortFile()
		if port <= 0 {
			port = defaultPort
		}
		if isServerRunning(port) {
			return port
		}
		time.Sleep(200 * time.Millisecond)
	}
	return 0
}

// openApp opens (or focuses) the Penpal desktop app.
func openApp() {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", "-a", "Penpal")
	case "linux":
		cmd = exec.Command("xdg-open", "penpal://")
	default:
		fmt.Fprintf(os.Stderr, "Error: unsupported platform %s\n", runtime.GOOS)
		return
	}
	if err := cmd.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not open Penpal app: %v\n", err)
		fmt.Fprintf(os.Stderr, "Is Penpal.app installed? Run: just install\n")
	}
}

// doGet performs an HTTP GET and prints the response body to stdout.
// E-PENPAL-CLI-AGENT-CMDS: shared GET helper for agent commands.
func doGet(url string) {
	resp, err := http.Get(url)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: could not contact server: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		fmt.Fprintf(os.Stderr, "Error: server returned %d: %s\n", resp.StatusCode, strings.TrimSpace(string(bodyBytes)))
		os.Exit(1)
	}

	io.Copy(os.Stdout, resp.Body)
	fmt.Fprintln(os.Stdout)
}

// requireFlags checks that session and project flags are provided, exits with usage if not.
func requireFlags(command, session, project string) {
	if session == "" || project == "" {
		fmt.Fprintf(os.Stderr, "Error: --session and --project are required for %s\n", command)
		os.Exit(1)
	}
}

// urlEncode encodes a string for use in a URL query parameter.
func urlEncode(s string) string {
	return url.QueryEscape(s)
}
