package agents

import (
	"sync"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/cache"
)

func TestStart_BlockedDuringCooldown(t *testing.T) {
	m := &Manager{
		agents:        make(map[string]*Agent),
		lastQuickExit: map[string]time.Time{"proj": time.Now()},
		cache:         cache.New(),
	}

	agent, err := m.Start("proj")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if agent != nil {
		t.Error("expected nil agent during cooldown, got non-nil")
	}
}

func TestStart_AllowedAfterCooldownExpires(t *testing.T) {
	m := &Manager{
		agents:        make(map[string]*Agent),
		lastQuickExit: map[string]time.Time{"proj": time.Now().Add(-restartCooldown - time.Second)},
		cache:         cache.New(),
	}

	// Start will fail because there's no real project in cache, but it should
	// get past the cooldown check and fail on the project lookup instead.
	_, err := m.Start("proj")
	if err == nil {
		t.Fatal("expected error from missing project, got nil")
	}
	// The error should be about the project not being found, not about cooldown
	if err.Error() != `project "proj" not found` {
		t.Errorf("unexpected error: %v", err)
	}
}

func TestStart_CooldownDoesNotAffectOtherProjects(t *testing.T) {
	m := &Manager{
		agents:        make(map[string]*Agent),
		lastQuickExit: map[string]time.Time{"proj-a": time.Now()},
		cache:         cache.New(),
	}

	// proj-b should not be blocked by proj-a's cooldown
	_, err := m.Start("proj-b")
	if err == nil {
		t.Fatal("expected error from missing project, got nil")
	}
	if err.Error() != `project "proj-b" not found` {
		t.Errorf("expected project-not-found error, got: %v", err)
	}
}

func TestStatus_ReturnsCooldownWhenNoAgent(t *testing.T) {
	m := &Manager{
		agents:        make(map[string]*Agent),
		lastQuickExit: map[string]time.Time{"proj": time.Now()},
	}

	status := m.Status("proj")
	if status == nil {
		t.Fatal("expected non-nil status during cooldown")
	}
	if !status.Cooldown {
		t.Error("expected Cooldown=true")
	}
	if status.Running {
		t.Error("expected Running=false during cooldown")
	}
	if status.Project != "proj" {
		t.Errorf("expected Project=proj, got %s", status.Project)
	}
}

func TestStatus_NoCooldownWhenExpired(t *testing.T) {
	m := &Manager{
		agents:        make(map[string]*Agent),
		lastQuickExit: map[string]time.Time{"proj": time.Now().Add(-restartCooldown - time.Second)},
	}

	status := m.Status("proj")
	if status != nil {
		t.Errorf("expected nil status after cooldown expires, got %+v", status)
	}
}

func TestStatus_NoCooldownWhenAgentRunning(t *testing.T) {
	a := newTestAgent()
	a.Project = "proj"
	a.done = make(chan struct{}) // open = running

	m := &Manager{
		agents:        map[string]*Agent{"proj": a},
		lastQuickExit: map[string]time.Time{"proj": time.Now()}, // cooldown active but agent is running
	}

	status := m.Status("proj")
	if status == nil {
		t.Fatal("expected non-nil status for running agent")
	}
	if status.Cooldown {
		t.Error("expected Cooldown=false when agent is running")
	}
	if !status.Running {
		t.Error("expected Running=true")
	}
}

func TestStatus_NilForUnknownProjectNoCooldown(t *testing.T) {
	m := &Manager{
		agents:        make(map[string]*Agent),
		lastQuickExit: make(map[string]time.Time),
	}

	status := m.Status("unknown")
	if status != nil {
		t.Errorf("expected nil status for unknown project, got %+v", status)
	}
}

func TestQuickExitThresholdAndCooldownConstants(t *testing.T) {
	// Sanity check: cooldown should be longer than the threshold
	if restartCooldown <= quickExitThreshold {
		t.Errorf("restartCooldown (%s) should be > quickExitThreshold (%s)", restartCooldown, quickExitThreshold)
	}
}

func TestStart_ReturnsNilForAlreadyRunningAgent(t *testing.T) {
	a := &Agent{
		Project: "proj",
		done:    make(chan struct{}), // open = running
	}

	m := &Manager{
		mu:            sync.Mutex{},
		agents:        map[string]*Agent{"proj": a},
		lastQuickExit: make(map[string]time.Time),
	}

	agent, err := m.Start("proj")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if agent != nil {
		t.Error("expected nil agent for already-running project")
	}
}
