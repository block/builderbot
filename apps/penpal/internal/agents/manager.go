package agents

import (
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
)

// Agent represents a running Claude Code agent process.
type Agent struct {
	Project     string // qualified project name
	ProjectPath string // absolute filesystem path (working directory)
	PID         int    // OS process ID
	StartedAt   time.Time
	cmd         *exec.Cmd
	done        chan struct{} // closed when process exits
	exitErr     error         // set after process exits

	mu            sync.Mutex
	contextWindow int     // from result message or default 200000
	contextUsed   int     // latest effective input tokens
	totalCostUSD  float64 // running cost
	numTurns      int     // number of assistant turns
}

// Manager manages Claude Code agent processes, one per project.
type Manager struct {
	mu        sync.Mutex
	agents    map[string]*Agent // key: qualified project name
	cache     *cache.Cache
	comments  *comments.Store
	port      int
	onChange  func(projectName string) // called when agent starts or stops
	claudeBin func() string            // returns resolved path to claude binary
	sm        *sessionManager           // E-PENPAL-SESSION-MGMT: external CLI session tracking
}

func New(c *cache.Cache, cs *comments.Store, port int) *Manager {
	return &Manager{
		agents:   make(map[string]*Agent),
		cache:    c,
		comments: cs,
		port:     port,
		sm:       newSessionManager(),
	}
}

// SetClaudeBin sets the function used to resolve the claude binary path.
func (m *Manager) SetClaudeBin(fn func() string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.claudeBin = fn
}

// SetOnChange sets a callback invoked when an agent starts or stops.
func (m *Manager) SetOnChange(fn func(projectName string)) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.onChange = fn
}

// Start launches a Claude agent for the given project.
// Returns nil, nil if a spawned agent is already running.
// Returns an error if a CLI session is active.
// E-PENPAL-AGENT-SPAWN: claims session, writes temp MCP config, builds prompt, runs claude.
func (m *Manager) Start(projectName string) (*Agent, error) {
	proj := m.cache.FindProject(projectName)
	if proj == nil {
		return nil, fmt.Errorf("project %q not found", projectName)
	}

	// Claim a spawned session — this handles all contention.
	// If a spawned session already exists (agent already running), claimSession
	// returns an error; we map that to the idempotent nil,nil return.
	// E-PENPAL-AGENT-SELF-ID: spawned agents are always claude.
	sess, err := m.claimSession(projectName, "", "claude", false, SessionSpawned)
	if err != nil {
		// Check if a spawned agent is already running — return nil,nil (idempotent).
		m.mu.Lock()
		if a, ok := m.agents[projectName]; ok {
			select {
			case <-a.done:
				// Agent exited but session not yet cleaned up — fall through to error.
				delete(m.agents, projectName)
			default:
				m.mu.Unlock()
				return nil, nil // already running
			}
		}
		m.mu.Unlock()
		return nil, err
	}

	// Build and launch the process outside the lock.
	agent, err := m.launchAgent(projectName, proj.Path, sess.Token)
	if err != nil {
		// Claim failed at launch — release the session.
		m.Detach(sess.Token)
		return nil, err
	}

	// Install the agent under the lock, verifying we still own the session.
	m.mu.Lock()
	currentToken, owned := m.sm.projectSession[projectName]
	if !owned || currentToken != sess.Token {
		// Someone else claimed the project while we were launching.
		m.mu.Unlock()
		agent.cmd.Process.Kill()
		<-agent.done
		return nil, fmt.Errorf("session was replaced during agent launch for %q", projectName)
	}
	m.agents[projectName] = agent
	m.mu.Unlock()

	log.Printf("Agent started for %s (PID %d)", projectName, agent.PID)

	if m.onChange != nil {
		go m.onChange(projectName)
	}

	return agent, nil
}

// launchAgent builds the temp MCP config and starts the claude process.
// Returns the Agent (with background goroutines running) or an error.
// The sessionToken is captured so the exit goroutine can clean up the right session.
func (m *Manager) launchAgent(projectName, projectPath, sessionToken string) (*Agent, error) {
	// Write temporary MCP config
	mcpConfigPath := filepath.Join(os.TempDir(),
		fmt.Sprintf("penpal-agent-%s.json", sanitize(projectName)))
	mcpConfig := fmt.Sprintf(`{"mcpServers":{"penpal":{"type":"http","url":"http://localhost:%d/mcp"}}}`, m.port)
	if err := os.WriteFile(mcpConfigPath, []byte(mcpConfig), 0644); err != nil {
		return nil, fmt.Errorf("write mcp config: %w", err)
	}

	prompt := buildPrompt(projectName)

	claudeBin := "claude"
	if m.claudeBin != nil {
		if p := m.claudeBin(); p != "" {
			claudeBin = p
		}
	}

	cmd := exec.Command(claudeBin,
		"-p", prompt,
		"--mcp-config", mcpConfigPath,
		"--dangerously-skip-permissions",
		"--verbose",
		"--output-format", "stream-json",
		"--max-budget-usd", "5",
		"--model", "opus",
	)
	cmd.Dir = projectPath

	// Log agent output to a file
	logPath := filepath.Join(projectPath, ".penpal", "agent.log")
	os.MkdirAll(filepath.Dir(logPath), 0755)
	logFile, err := os.Create(logPath)
	if err != nil {
		return nil, fmt.Errorf("create log file: %w", err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		logFile.Close()
		return nil, fmt.Errorf("stdout pipe: %w", err)
	}
	cmd.Stderr = logFile

	if err := cmd.Start(); err != nil {
		logFile.Close()
		return nil, fmt.Errorf("start claude: %w", err)
	}

	agent := &Agent{
		Project:       projectName,
		ProjectPath:   projectPath,
		PID:           cmd.Process.Pid,
		StartedAt:     time.Now(),
		cmd:           cmd,
		done:          make(chan struct{}),
		contextWindow: 200000, // default for opus, refined when result arrives
	}

	// Parse NDJSON stream in background, writing through to log file.
	streamDone := make(chan struct{})
	go func() {
		agent.parseStream(stdout, logFile)
		close(streamDone)
	}()

	// Monitor process exit in background.
	go func() {
		agent.exitErr = cmd.Wait()
		<-streamDone
		logFile.Close()
		os.Remove(mcpConfigPath)
		close(agent.done)
		log.Printf("Agent exited for %s (PID %d): %v", projectName, agent.PID, agent.exitErr)

		m.comments.ClearProjectWorking(projectName)

		m.mu.Lock()
		if current, ok := m.agents[projectName]; ok && current == agent {
			delete(m.agents, projectName)
		}
		fn := m.onChange
		m.mu.Unlock()

		// Clean up the session by token (safe even if already replaced).
		m.Detach(sessionToken)

		if fn != nil {
			fn(projectName)
		}
	}()

	return agent, nil
}

// Stop terminates the agent for the given project.
// E-PENPAL-AGENT-CLEANUP: graceful shutdown with SIGTERM, falls back to kill.
func (m *Manager) Stop(projectName string) error {
	m.mu.Lock()
	agent, ok := m.agents[projectName]
	if !ok {
		m.mu.Unlock()
		return fmt.Errorf("no agent running for %q", projectName)
	}
	m.mu.Unlock()

	// Send SIGTERM for graceful shutdown
	if err := agent.cmd.Process.Signal(os.Interrupt); err != nil {
		// Process may have already exited
		return nil
	}

	// Wait up to 5 seconds for clean exit
	select {
	case <-agent.done:
		return nil
	case <-time.After(5 * time.Second):
		agent.cmd.Process.Kill()
		<-agent.done
		return nil
	}
}

// AgentStatus contains the status of an agent for a project.
// E-PENPAL-AGENT-STREAM: context/cost fields populated from NDJSON stdout parsing.
type AgentStatus struct {
	Project        string    `json:"project"`
	PID            int       `json:"pid"`
	StartedAt      time.Time `json:"startedAt"`
	Running        bool      `json:"running"`
	ContextWindow  int       `json:"contextWindow"`
	ContextUsed    int       `json:"contextUsed"`
	ContextPercent float64   `json:"contextPercent"`
	TotalCostUSD   float64   `json:"totalCostUSD"`
	NumTurns       int       `json:"numTurns"`
}

// Status returns the agent status for a project, or nil if no agent.
func (m *Manager) Status(projectName string) *AgentStatus {
	m.mu.Lock()
	defer m.mu.Unlock()

	agent, ok := m.agents[projectName]
	if !ok {
		return nil
	}

	running := true
	select {
	case <-agent.done:
		running = false
	default:
	}

	agent.mu.Lock()
	contextWindow := agent.contextWindow
	contextUsed := agent.contextUsed
	totalCostUSD := agent.totalCostUSD
	numTurns := agent.numTurns
	agent.mu.Unlock()

	var contextPercent float64
	if contextWindow > 0 {
		contextPercent = float64(contextUsed) / float64(contextWindow) * 100
	}

	return &AgentStatus{
		Project:        agent.Project,
		PID:            agent.PID,
		StartedAt:      agent.StartedAt,
		Running:        running,
		ContextWindow:  contextWindow,
		ContextUsed:    contextUsed,
		ContextPercent: contextPercent,
		TotalCostUSD:   totalCostUSD,
		NumTurns:       numTurns,
	}
}

// SimulateFinished inserts a synthetic agent entry that appears to have
// already exited. This is intended for testing the "agent finished" status
// path without requiring an external binary.
func (m *Manager) SimulateFinished(projectName string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	done := make(chan struct{})
	close(done)
	m.agents[projectName] = &Agent{
		Project:       projectName,
		StartedAt:     time.Now(),
		done:          done,
		contextWindow: 200000,
	}
}

// AgentName returns the agent name from the active session for a project.
// Returns "agent" if no active session is found.
// E-PENPAL-AGENT-SELF-ID: used by MCP tools to derive comment author from session.
func (m *Manager) AgentName(projectName string) string {
	m.mu.Lock()
	defer m.mu.Unlock()

	token, ok := m.sm.projectSession[projectName]
	if !ok {
		return "agent"
	}
	sess, exists := m.sm.sessions[token]
	if !exists || sess.Evicted || sess.isExpired() {
		return "agent"
	}
	if sess.AgentName == "" {
		return "agent"
	}
	return sess.AgentName
}

// SimulateRunning inserts a synthetic agent entry that appears to be
// actively running, with an associated spawned session.
func (m *Manager) SimulateRunning(projectName string, contextUsed, contextWindow int, totalCostUSD float64, numTurns int) {
	m.mu.Lock()
	defer m.mu.Unlock()

	now := time.Now()
	sess := &Session{
		Token:         generateToken(),
		Project:       projectName,
		AgentName:     "claude",
		Kind:          SessionSpawned,
		CreatedAt:     now,
		LastHeartbeat: now,
	}
	m.sm.sessions[sess.Token] = sess
	m.sm.projectSession[projectName] = sess.Token

	m.agents[projectName] = &Agent{
		Project:       projectName,
		StartedAt:     now,
		PID:           99999,
		done:          make(chan struct{}), // not closed = still running
		contextWindow: contextWindow,
		contextUsed:   contextUsed,
		totalCostUSD:  totalCostUSD,
		numTurns:      numTurns,
	}
}

// StopAll terminates all running agents (for server shutdown).
func (m *Manager) StopAll() {
	m.mu.Lock()
	names := make([]string, 0, len(m.agents))
	for name := range m.agents {
		names = append(names, name)
	}
	m.mu.Unlock()

	for _, name := range names {
		m.Stop(name)
	}
}
