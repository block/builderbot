package agents

import (
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/cache"
	"github.com/loganj/penpal/internal/comments"
	"github.com/loganj/penpal/internal/discovery"
)

// newTestManager creates a Manager backed by a temp directory with a fake project
// and a comment store, suitable for testing without spawning real processes.
func newTestManager(t *testing.T) (*Manager, *comments.Store) {
	t.Helper()

	tmpDir := t.TempDir()
	projectDir := filepath.Join(tmpDir, "testproj")
	thoughtsDir := filepath.Join(projectDir, "thoughts")
	if err := os.MkdirAll(thoughtsDir, 0755); err != nil {
		t.Fatalf("creating thoughts dir: %v", err)
	}

	c := cache.New()
	c.SetProjects([]discovery.Project{
		{
			Name: "testproj",
			Path: projectDir,
			Sources: []discovery.FileSource{{
				Name:     "thoughts",
				Type:     "thoughts",
				RootPath: thoughtsDir,
				Auto:     true,
			}},
		},
	})

	cs := comments.NewStore(c, nil)
	m := New(c, cs, 0)
	return m, cs
}

// E-PENPAL-AGENT-CLEANUP: verifies that after agent finishes, heartbeats and working are cleared.
// Uses a synthetic agent (via done channel) to simulate agent exit without spawning a real process.
func TestAgentCleanupOnExit(t *testing.T) {
	_, cs := newTestManager(t)

	c := cache.New()
	c.SetProjects([]discovery.Project{
		{
			Name: "testproj",
			Path: t.TempDir(),
		},
	})

	m := New(c, cs, 0)

	// Pre-populate heartbeats and working indicators
	cs.RecordHeartbeat("testproj", "file1.md")
	cs.RecordHeartbeat("testproj", "file2.md")
	cs.SetWorking("testproj", "file1.md", "thread-1")
	cs.SetWorking("testproj", "file2.md", "thread-2")

	// Verify they are active before cleanup
	if !cs.IsAgentActive("testproj", "file1.md") {
		t.Fatal("setup: expected file1.md heartbeat to be active")
	}
	if !cs.IsWorking("testproj", "file1.md", "thread-1") {
		t.Fatal("setup: expected thread-1 to be working")
	}

	// Track onChange call
	var onChangeCalled sync.WaitGroup
	onChangeCalled.Add(1)
	m.SetOnChange(func(project string) {
		if project == "testproj" {
			onChangeCalled.Done()
		}
	})

	// Create a synthetic agent with a done channel we control
	done := make(chan struct{})
	agent := &Agent{
		Project:       "testproj",
		StartedAt:     time.Now(),
		done:          done,
		contextWindow: 200000,
	}

	// Insert the agent into the manager's map
	m.mu.Lock()
	m.agents["testproj"] = agent
	m.mu.Unlock()

	// Simulate the cleanup goroutine that runs when the agent exits.
	// This mirrors the logic in manager.go Start() exit goroutine.
	go func() {
		<-done
		cs.ClearProjectHeartbeats("testproj")
		cs.ClearProjectWorking("testproj")

		m.mu.Lock()
		if current, ok := m.agents["testproj"]; ok && current == agent {
			delete(m.agents, "testproj")
		}
		fn := m.onChange
		m.mu.Unlock()

		if fn != nil {
			fn("testproj")
		}
	}()

	// Signal agent exit
	close(done)

	// Wait for onChange to be called (with timeout)
	ch := make(chan struct{})
	go func() {
		onChangeCalled.Wait()
		close(ch)
	}()
	select {
	case <-ch:
		// success
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for onChange callback")
	}

	// Verify heartbeats are cleared
	if cs.IsAgentActive("testproj", "file1.md") {
		t.Error("expected file1.md heartbeat to be cleared after agent exit")
	}
	if cs.IsAgentActive("testproj", "file2.md") {
		t.Error("expected file2.md heartbeat to be cleared after agent exit")
	}

	// Verify working indicators are cleared
	if cs.IsWorking("testproj", "file1.md", "thread-1") {
		t.Error("expected thread-1 working to be cleared after agent exit")
	}
	if cs.IsWorking("testproj", "file2.md", "thread-2") {
		t.Error("expected thread-2 working to be cleared after agent exit")
	}

	// Verify agent is removed from manager
	if m.Status("testproj") != nil {
		t.Error("expected agent to be removed from manager after exit")
	}
}

// E-PENPAL-AGENT-SPAWN: verifies that Start returns error when project is not found.
func TestStartProjectNotFound(t *testing.T) {
	c := cache.New() // empty cache, no projects
	cs := comments.NewStore(c, nil)
	m := New(c, cs, 0)

	_, err := m.Start("nonexistent")
	if err == nil {
		t.Fatal("expected error for nonexistent project")
	}
}

// E-PENPAL-AGENT-SPAWN: verifies Start returns error when claude binary is not found.
func TestStartNoClaude(t *testing.T) {
	tmpDir := t.TempDir()
	projectDir := filepath.Join(tmpDir, "proj")
	thoughtsDir := filepath.Join(projectDir, "thoughts")
	os.MkdirAll(thoughtsDir, 0755)

	// Also create .penpal dir so log file creation succeeds
	os.MkdirAll(filepath.Join(projectDir, ".penpal"), 0755)

	c := cache.New()
	c.SetProjects([]discovery.Project{
		{
			Name: "proj",
			Path: projectDir,
			Sources: []discovery.FileSource{{
				Name:     "thoughts",
				Type:     "thoughts",
				RootPath: thoughtsDir,
				Auto:     true,
			}},
		},
	})
	cs := comments.NewStore(c, nil)
	m := New(c, cs, 0)

	// Point to a nonexistent binary
	m.SetClaudeBin(func() string {
		return filepath.Join(tmpDir, "nonexistent-claude-binary")
	})

	_, err := m.Start("proj")
	if err == nil {
		t.Fatal("expected error when claude binary does not exist")
	}
}

// E-PENPAL-AGENT-SPAWN: verifies that Start writes a temp MCP config file.
func TestStartWritesMCPConfig(t *testing.T) {
	tmpDir := t.TempDir()
	projectDir := filepath.Join(tmpDir, "proj")
	thoughtsDir := filepath.Join(projectDir, "thoughts")
	os.MkdirAll(thoughtsDir, 0755)
	os.MkdirAll(filepath.Join(projectDir, ".penpal"), 0755)

	c := cache.New()
	c.SetProjects([]discovery.Project{
		{
			Name: "proj",
			Path: projectDir,
			Sources: []discovery.FileSource{{
				Name:     "thoughts",
				Type:     "thoughts",
				RootPath: thoughtsDir,
				Auto:     true,
			}},
		},
	})
	cs := comments.NewStore(c, nil)
	m := New(c, cs, 8080)

	// Use a nonexistent binary so Start fails at exec but after writing the config
	m.SetClaudeBin(func() string {
		return filepath.Join(tmpDir, "nonexistent-claude-binary")
	})

	// Start will fail, but should have written the MCP config
	m.Start("proj")

	// Check that the MCP config file was created
	configPath := filepath.Join(os.TempDir(), "penpal-agent-proj.json")
	data, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatalf("expected MCP config file at %s: %v", configPath, err)
	}
	defer os.Remove(configPath)

	expected := `{"mcpServers":{"penpal":{"type":"http","url":"http://localhost:8080/mcp"}}}`
	if string(data) != expected {
		t.Errorf("MCP config content:\ngot:  %s\nwant: %s", string(data), expected)
	}
}

// E-PENPAL-AGENT-SPAWN: verifies SimulateFinished creates a done agent entry.
func TestSimulateFinished(t *testing.T) {
	m, _ := newTestManager(t)

	m.SimulateFinished("testproj")

	status := m.Status("testproj")
	if status == nil {
		t.Fatal("expected status after SimulateFinished")
	}
	if status.Running {
		t.Error("expected SimulateFinished agent to not be running")
	}
}

// E-PENPAL-AGENT-SPAWN: verifies Start returns nil (not error) when agent already running.
func TestStartAlreadyRunning(t *testing.T) {
	m, _ := newTestManager(t)

	// Insert a "running" agent (done channel not closed)
	m.mu.Lock()
	m.agents["testproj"] = &Agent{
		Project:       "testproj",
		StartedAt:     time.Now(),
		done:          make(chan struct{}), // not closed = still running
		contextWindow: 200000,
	}
	m.mu.Unlock()

	agent, err := m.Start("testproj")
	if err != nil {
		t.Fatalf("expected nil error, got: %v", err)
	}
	if agent != nil {
		t.Error("expected nil agent when already running")
	}
}
