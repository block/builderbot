package agents

import (
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
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
}

// Manager manages Claude Code agent processes, one per project.
type Manager struct {
	mu       sync.Mutex
	agents   map[string]*Agent // key: qualified project name
	cache    *cache.Cache
	comments *comments.Store
	port     int
	onChange func() // called when agent starts or stops
}

func New(c *cache.Cache, cs *comments.Store, port int) *Manager {
	return &Manager{
		agents:   make(map[string]*Agent),
		cache:    c,
		comments: cs,
		port:     port,
	}
}

// SetOnChange sets a callback invoked when an agent starts or stops.
func (m *Manager) SetOnChange(fn func()) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.onChange = fn
}

// Start launches a Claude agent for the given project.
// Returns nil if an agent is already running for this project.
func (m *Manager) Start(projectName string) (*Agent, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if a, ok := m.agents[projectName]; ok {
		select {
		case <-a.done:
			// Previous agent exited, clean up and proceed
			delete(m.agents, projectName)
		default:
			return nil, nil // already running
		}
	}

	proj := m.cache.FindProject(projectName)
	if proj == nil {
		return nil, fmt.Errorf("project %q not found", projectName)
	}

	// Write temporary MCP config
	mcpConfigPath := filepath.Join(os.TempDir(),
		fmt.Sprintf("birdseye-agent-%s.json", sanitize(projectName)))
	mcpConfig := fmt.Sprintf(`{"mcpServers":{"birdseye":{"type":"http","url":"http://localhost:%d/mcp"}}}`, m.port)
	if err := os.WriteFile(mcpConfigPath, []byte(mcpConfig), 0644); err != nil {
		return nil, fmt.Errorf("write mcp config: %w", err)
	}

	prompt := buildPrompt(projectName)

	cmd := exec.Command("claude",
		"-p", prompt,
		"--mcp-config", mcpConfigPath,
		"--dangerously-skip-permissions",
		"--verbose",
		"--output-format", "stream-json",
		"--max-budget-usd", "5",
		"--model", "opus",
	)
	cmd.Dir = proj.Path

	// Log agent output to a file
	logPath := filepath.Join(proj.Path, ".birdseye", "agent.log")
	os.MkdirAll(filepath.Dir(logPath), 0755)
	logFile, err := os.Create(logPath)
	if err != nil {
		return nil, fmt.Errorf("create log file: %w", err)
	}
	cmd.Stdout = logFile
	cmd.Stderr = logFile

	if err := cmd.Start(); err != nil {
		logFile.Close()
		return nil, fmt.Errorf("start claude: %w", err)
	}

	agent := &Agent{
		Project:     projectName,
		ProjectPath: proj.Path,
		PID:         cmd.Process.Pid,
		StartedAt:   time.Now(),
		cmd:         cmd,
		done:        make(chan struct{}),
	}
	m.agents[projectName] = agent

	log.Printf("Agent started for %s (PID %d)", projectName, agent.PID)

	// Monitor process exit in background
	go func() {
		agent.exitErr = cmd.Wait()
		logFile.Close()
		os.Remove(mcpConfigPath)
		close(agent.done)
		log.Printf("Agent exited for %s (PID %d): %v", projectName, agent.PID, agent.exitErr)

		m.mu.Lock()
		// Only delete if it's still the same agent (not replaced)
		if current, ok := m.agents[projectName]; ok && current == agent {
			delete(m.agents, projectName)
		}
		fn := m.onChange
		m.mu.Unlock()

		if fn != nil {
			fn()
		}
	}()

	if m.onChange != nil {
		go m.onChange()
	}

	return agent, nil
}

// Stop terminates the agent for the given project.
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
type AgentStatus struct {
	Project   string    `json:"project"`
	PID       int       `json:"pid"`
	StartedAt time.Time `json:"startedAt"`
	Running   bool      `json:"running"`
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

	return &AgentStatus{
		Project:   agent.Project,
		PID:       agent.PID,
		StartedAt: agent.StartedAt,
		Running:   running,
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
