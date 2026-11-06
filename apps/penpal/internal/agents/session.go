package agents

import (
	"crypto/rand"
	"fmt"
	"math/big"
	"time"
)

const sessionTimeout = 90 * time.Second

// SessionKind distinguishes how a session was created.
type SessionKind int

const (
	// SessionCLI is an external CLI-attached agent session with heartbeat expiry.
	SessionCLI SessionKind = iota
	// SessionSpawned is a session owned by a Manager-launched process (no heartbeat expiry).
	SessionSpawned
)

// Session represents an agent session (spawned or CLI-attached).
// E-PENPAL-SESSION-MGMT: tracks token, project, worktree, heartbeat, eviction, kind, and agent name.
type Session struct {
	Token         string
	Project       string // qualified project name
	Worktree      string // may be empty
	AgentName     string // self-reported agent name (e.g., "amp", "claude")
	Kind          SessionKind
	CreatedAt     time.Time
	LastHeartbeat time.Time
	Evicted       bool
}

// isExpired reports whether the session's last heartbeat is older than sessionTimeout.
// E-PENPAL-SESSION-MGMT: lazy expiration check on access.
func (s *Session) isExpired() bool {
	if s.Kind == SessionSpawned {
		return false // spawned sessions live until process exits
	}
	return time.Since(s.LastHeartbeat) > sessionTimeout
}

// sessionManager holds external CLI-attached agent session state.
// All fields are protected by Manager.mu (no separate mutex — consistent
// lock ordering eliminates ABBA deadlock risk).
// E-PENPAL-CLI-ATTACH: embedded in Manager for session tracking.
type sessionManager struct {
	sessions       map[string]*Session // token -> session
	projectSession map[string]string   // project name -> token
}

func newSessionManager() *sessionManager {
	return &sessionManager{
		sessions:       make(map[string]*Session),
		projectSession: make(map[string]string),
	}
}

// generateToken produces a UUID-style random token using crypto/rand.
// E-PENPAL-SESSION-MGMT: secure token generation for session identity.
func generateToken() string {
	const alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
	b := make([]byte, 32)
	for i := range b {
		n, _ := rand.Int(rand.Reader, big.NewInt(int64(len(alphabet))))
		b[i] = alphabet[n.Int64()]
	}
	return string(b)
}

// Attach creates an external CLI agent session for the given project.
// If force is false and any agent (spawned or CLI) is active, an error is returned.
// If force is true, existing agents/sessions are evicted.
// E-PENPAL-CLI-ATTACH: creates session, checks contention, evicts if forced.
// E-PENPAL-AGENT-SELF-ID: stores the agent's self-reported name on the session.
func (m *Manager) Attach(projectName, worktree, agentName string, force bool) (*Session, error) {
	return m.claimSession(projectName, worktree, agentName, force, SessionCLI)
}

// claimSession is the unified contention + session creation path used by both
// Attach (CLI) and Start (spawned). It evicts any prior owner when force is
// true, then creates a new Session of the given kind.
// Caller must NOT hold m.mu.
// E-PENPAL-SESSION-MGMT: unified session claim for all agent types.
// E-PENPAL-AGENT-SELF-ID: stores agent name on the session.
func (m *Manager) claimSession(projectName, worktree, agentName string, force bool, kind SessionKind) (*Session, error) {
	m.mu.Lock()

	// Check for spawned agent contention.
	if agent, ok := m.agents[projectName]; ok {
		running := true
		select {
		case <-agent.done:
			running = false
			delete(m.agents, projectName)
		default:
		}
		if running {
			if !force {
				m.mu.Unlock()
				return nil, fmt.Errorf("agent already running for %q (PID %d)", projectName, agent.PID)
			}
			// Force-evict: stop the spawned agent outside the lock
			// (Stop acquires m.mu internally).
			m.mu.Unlock()
			m.Stop(projectName)
			m.mu.Lock()
			// Re-verify agent is gone after re-acquiring lock.
			if agent, ok := m.agents[projectName]; ok {
				select {
				case <-agent.done:
					delete(m.agents, projectName)
				default:
					m.mu.Unlock()
					return nil, fmt.Errorf("failed to stop agent for %q", projectName)
				}
			}
		}
	}
	// From here on, m.mu is held until the end of the function.
	defer m.mu.Unlock()

	// Check for existing session contention.
	if token, ok := m.sm.projectSession[projectName]; ok {
		if sess, exists := m.sm.sessions[token]; exists {
			if !sess.Evicted && !sess.isExpired() {
				if !force {
					return nil, fmt.Errorf("agent session already active for %q", projectName)
				}
				sess.Evicted = true
			}
			delete(m.sm.sessions, token)
			delete(m.sm.projectSession, projectName)
		}
	}

	// Create new session.
	now := time.Now()
	sess := &Session{
		Token:         generateToken(),
		Project:       projectName,
		Worktree:      worktree,
		AgentName:     agentName,
		Kind:          kind,
		CreatedAt:     now,
		LastHeartbeat: now,
	}

	m.sm.sessions[sess.Token] = sess
	m.sm.projectSession[projectName] = sess.Token

	// Notify onChange for CLI sessions immediately. For spawned sessions,
	// Start() fires onChange after the process is installed.
	if kind == SessionCLI {
		fn := m.onChange
		if fn != nil {
			go fn(projectName)
		}
	}

	return sess, nil
}

// ValidateSession returns the session for the given token, or an error if the
// session is not found, evicted, or expired.
// E-PENPAL-SESSION-MGMT: validates token and performs lazy expiration cleanup.
func (m *Manager) ValidateSession(token string) (*Session, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	sess, ok := m.sm.sessions[token]
	if !ok {
		return nil, fmt.Errorf("session not found")
	}
	if sess.Evicted {
		delete(m.sm.sessions, token)
		delete(m.sm.projectSession, sess.Project)
		return nil, fmt.Errorf("session was evicted")
	}
	if sess.isExpired() {
		delete(m.sm.sessions, token)
		delete(m.sm.projectSession, sess.Project)
		return nil, fmt.Errorf("session expired")
	}
	return sess, nil
}

// RecordSessionHeartbeat updates the LastHeartbeat for the given session token.
// E-PENPAL-SESSION-MGMT: heartbeat keeps session alive, prevents expiration.
func (m *Manager) RecordSessionHeartbeat(token string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if sess, ok := m.sm.sessions[token]; ok && !sess.Evicted {
		sess.LastHeartbeat = time.Now()
	}
}

// HasActiveAgent reports whether any active (non-expired, non-evicted) session
// exists for the given project.
// E-PENPAL-AGENT-PARITY: unified check — session is the single source of truth.
func (m *Manager) HasActiveAgent(projectName string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()

	token, ok := m.sm.projectSession[projectName]
	if !ok {
		return false
	}
	sess, exists := m.sm.sessions[token]
	if !exists {
		delete(m.sm.projectSession, projectName)
		return false
	}
	if sess.Evicted || sess.isExpired() {
		delete(m.sm.sessions, token)
		delete(m.sm.projectSession, projectName)
		return false
	}
	return true
}

// StopAny stops any active agent for the project — spawned or CLI-attached.
// E-PENPAL-CLI-CONTENTION: unified stop for both agent types.
func (m *Manager) StopAny(projectName string) {
	m.mu.Lock()
	hasAgent := false
	if _, ok := m.agents[projectName]; ok {
		hasAgent = true
	}

	// Evict any session (spawned or CLI).
	if token, ok := m.sm.projectSession[projectName]; ok {
		if sess, exists := m.sm.sessions[token]; exists {
			sess.Evicted = true
		}
		delete(m.sm.sessions, token)
		delete(m.sm.projectSession, projectName)
	}

	fn := m.onChange
	m.mu.Unlock()

	// Stop spawned agent outside m.mu (Stop acquires m.mu internally).
	if hasAgent {
		m.Stop(projectName)
	}

	if fn != nil {
		fn(projectName)
	}
}

// Detach removes a session cleanly (spawned or CLI).
// E-PENPAL-CLI-ATTACH: clean session teardown.
func (m *Manager) Detach(token string) {
	m.mu.Lock()
	sess, ok := m.sm.sessions[token]
	if !ok {
		m.mu.Unlock()
		return
	}
	projectName := sess.Project
	delete(m.sm.sessions, token)
	delete(m.sm.projectSession, projectName)
	fn := m.onChange
	m.mu.Unlock()

	if fn != nil {
		fn(projectName)
	}
}
