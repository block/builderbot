package agents

import (
	"testing"
	"time"
)

// E-PENPAL-CLI-ATTACH: verifies Attach creates a valid session.
func TestAttach_CreatesSession(t *testing.T) {
	m, _ := newTestManager(t)

	sess, err := m.Attach("testproj", "", "claude", false)
	if err != nil {
		t.Fatalf("Attach: %v", err)
	}
	if sess.Token == "" {
		t.Error("expected non-empty token")
	}
	if sess.Project != "testproj" {
		t.Errorf("expected project=testproj, got %q", sess.Project)
	}
	if sess.Evicted {
		t.Error("expected Evicted=false")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies Attach fails when another session is active.
func TestAttach_Conflict_WithoutForce(t *testing.T) {
	m, _ := newTestManager(t)

	_, err := m.Attach("testproj", "", "claude", false)
	if err != nil {
		t.Fatalf("first Attach: %v", err)
	}

	_, err = m.Attach("testproj", "", "claude", false)
	if err == nil {
		t.Fatal("expected conflict error on second Attach without force")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies Attach with force evicts existing session.
func TestAttach_Force_EvictsSession(t *testing.T) {
	m, _ := newTestManager(t)

	sess1, err := m.Attach("testproj", "", "claude", false)
	if err != nil {
		t.Fatalf("first Attach: %v", err)
	}

	sess2, err := m.Attach("testproj", "", "claude", true)
	if err != nil {
		t.Fatalf("forced Attach: %v", err)
	}

	if sess2.Token == sess1.Token {
		t.Error("expected different token after force-evict")
	}

	// Validate the evicted session should fail.
	if _, err := m.ValidateSession(sess1.Token); err == nil {
		t.Error("expected evicted session to fail validation")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies Attach fails when a spawned agent is running.
func TestAttach_ConflictWithSpawnedAgent(t *testing.T) {
	m, _ := newTestManager(t)

	m.SimulateRunning("testproj", 1000, 200000, 0.5, 1)

	// Without force, Attach should fail because a spawned agent is running.
	_, err := m.Attach("testproj", "", "claude", false)
	if err == nil {
		t.Fatal("expected error when spawned agent is running")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies Attach with force succeeds after spawned agent exits.
func TestAttach_Force_AfterSpawnedAgentExits(t *testing.T) {
	m, _ := newTestManager(t)

	// Insert a finished (exited) agent so force-eviction can clean it up.
	m.SimulateFinished("testproj")

	sess, err := m.Attach("testproj", "", "claude", true)
	if err != nil {
		t.Fatalf("forced Attach after exited agent: %v", err)
	}
	if sess.Token == "" {
		t.Error("expected valid session")
	}
}

// E-PENPAL-SESSION-MGMT: verifies ValidateSession returns session for valid token.
func TestValidateSession_Valid(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "wt1", "claude", false)

	validated, err := m.ValidateSession(sess.Token)
	if err != nil {
		t.Fatalf("ValidateSession: %v", err)
	}
	if validated.Project != "testproj" {
		t.Errorf("expected project=testproj, got %q", validated.Project)
	}
	if validated.Worktree != "wt1" {
		t.Errorf("expected worktree=wt1, got %q", validated.Worktree)
	}
}

// E-PENPAL-SESSION-MGMT: verifies ValidateSession returns error for evicted session.
func TestValidateSession_Evicted(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "", "claude", false)

	// Evict by forcing a new session.
	m.Attach("testproj", "", "claude", true)

	_, err := m.ValidateSession(sess.Token)
	if err == nil {
		t.Fatal("expected error for evicted session")
	}
}

// E-PENPAL-SESSION-MGMT: verifies ValidateSession returns error for expired session.
func TestValidateSession_Expired(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "", "claude", false)

	// Directly set LastHeartbeat to the past to simulate expiration.
	m.mu.Lock()
	sess.LastHeartbeat = time.Now().Add(-2 * sessionTimeout)
	m.mu.Unlock()

	_, err := m.ValidateSession(sess.Token)
	if err == nil {
		t.Fatal("expected error for expired session")
	}
}

// E-PENPAL-SESSION-MGMT: verifies ValidateSession returns error for unknown token.
func TestValidateSession_NotFound(t *testing.T) {
	m, _ := newTestManager(t)

	_, err := m.ValidateSession("nonexistent-token")
	if err == nil {
		t.Fatal("expected error for unknown token")
	}
}

// E-PENPAL-AGENT-ACTIVE-UNIFIED: verifies HasActiveAgent returns true for spawned agent.
func TestHasActiveAgent_SpawnedAgent(t *testing.T) {
	m, _ := newTestManager(t)

	m.SimulateRunning("testproj", 1000, 200000, 0.5, 1)

	if !m.HasActiveAgent("testproj") {
		t.Error("expected HasActiveAgent=true for spawned agent")
	}
}

// E-PENPAL-AGENT-ACTIVE-UNIFIED: verifies HasActiveAgent returns true for CLI session.
func TestHasActiveAgent_CLISession(t *testing.T) {
	m, _ := newTestManager(t)

	m.Attach("testproj", "", "claude", false)

	if !m.HasActiveAgent("testproj") {
		t.Error("expected HasActiveAgent=true for CLI session")
	}
}

// E-PENPAL-AGENT-ACTIVE-UNIFIED: verifies HasActiveAgent returns false when nothing active.
func TestHasActiveAgent_NoAgent(t *testing.T) {
	m, _ := newTestManager(t)

	if m.HasActiveAgent("testproj") {
		t.Error("expected HasActiveAgent=false when nothing active")
	}
}

// E-PENPAL-AGENT-ACTIVE-UNIFIED: verifies HasActiveAgent returns false for expired session.
func TestHasActiveAgent_ExpiredSession(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "", "claude", false)

	// Expire the session.
	m.mu.Lock()
	sess.LastHeartbeat = time.Now().Add(-2 * sessionTimeout)
	m.mu.Unlock()

	if m.HasActiveAgent("testproj") {
		t.Error("expected HasActiveAgent=false for expired session")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies StopAny evicts a CLI session.
func TestStopAny_EvictsCLISession(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "", "claude", false)

	m.StopAny("testproj")

	_, err := m.ValidateSession(sess.Token)
	if err == nil {
		t.Fatal("expected session to be evicted after StopAny")
	}
}

// E-PENPAL-CLI-CONTENTION: verifies StopAny on empty project doesn't panic.
func TestStopAny_NoAgent(t *testing.T) {
	m, _ := newTestManager(t)

	// Should not panic.
	m.StopAny("testproj")
}

// E-PENPAL-SESSION-MGMT: verifies RecordSessionHeartbeat updates LastHeartbeat.
func TestRecordSessionHeartbeat(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "", "claude", false)

	before := sess.LastHeartbeat
	time.Sleep(5 * time.Millisecond)

	m.RecordSessionHeartbeat(sess.Token)

	m.mu.Lock()
	after := sess.LastHeartbeat
	m.mu.Unlock()

	if !after.After(before) {
		t.Error("expected LastHeartbeat to be updated after RecordSessionHeartbeat")
	}
}

// E-PENPAL-AGENT-SELF-ID: verifies Attach stores AgentName on the session.
func TestAttach_StoresAgentName(t *testing.T) {
	m, _ := newTestManager(t)

	sess, err := m.Attach("testproj", "", "amp", false)
	if err != nil {
		t.Fatalf("Attach: %v", err)
	}
	if sess.AgentName != "amp" {
		t.Errorf("expected AgentName=amp, got %q", sess.AgentName)
	}
}

// E-PENPAL-AGENT-SELF-ID: verifies AgentName returns the session's agent name.
func TestAgentName_ReturnsSessionName(t *testing.T) {
	m, _ := newTestManager(t)

	m.Attach("testproj", "", "amp", false)

	if got := m.AgentName("testproj"); got != "amp" {
		t.Errorf("AgentName = %q, want %q", got, "amp")
	}
}

// E-PENPAL-AGENT-SELF-ID: verifies AgentName returns "agent" when no session exists.
func TestAgentName_DefaultsToAgent(t *testing.T) {
	m, _ := newTestManager(t)

	if got := m.AgentName("testproj"); got != "agent" {
		t.Errorf("AgentName = %q, want %q", got, "agent")
	}
}

// E-PENPAL-AGENT-SELF-ID: verifies SimulateRunning sets AgentName to "claude".
func TestSimulateRunning_SetsAgentName(t *testing.T) {
	m, _ := newTestManager(t)

	m.SimulateRunning("testproj", 1000, 200000, 0.5, 1)

	if got := m.AgentName("testproj"); got != "claude" {
		t.Errorf("AgentName = %q, want %q", got, "claude")
	}
}

// E-PENPAL-CLI-ATTACH: verifies Detach removes the session cleanly.
func TestDetach_RemovesSession(t *testing.T) {
	m, _ := newTestManager(t)

	sess, _ := m.Attach("testproj", "", "claude", false)

	m.Detach(sess.Token)

	if m.HasActiveAgent("testproj") {
		t.Error("expected no active agent after Detach")
	}

	_, err := m.ValidateSession(sess.Token)
	if err == nil {
		t.Fatal("expected session to be gone after Detach")
	}
}
