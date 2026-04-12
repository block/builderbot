package server

import (
	"bufio"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/loganj/penpal/internal/watcher"
)

// E-PENPAL-SSE: verifies SSE endpoint returns correct Content-Type and Cache-Control headers.
func TestSSE_Headers(t *testing.T) {
	s, _, _ := testServer(t)
	ts := httptest.NewServer(s)
	defer ts.Close()

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(ts.URL + "/events")
	if err != nil {
		t.Fatalf("GET /events: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		t.Fatalf("expected status 200, got %d", resp.StatusCode)
	}
	if ct := resp.Header.Get("Content-Type"); ct != "text/event-stream" {
		t.Errorf("expected Content-Type text/event-stream, got %q", ct)
	}
	if cc := resp.Header.Get("Cache-Control"); cc != "no-cache" {
		t.Errorf("expected Cache-Control no-cache, got %q", cc)
	}
}

// E-PENPAL-SSE: verifies SSE endpoint sends initial connected event on connection.
func TestSSE_ConnectedEvent(t *testing.T) {
	s, _, _ := testServer(t)
	ts := httptest.NewServer(s)
	defer ts.Close()

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(ts.URL + "/events")
	if err != nil {
		t.Fatalf("GET /events: %v", err)
	}
	defer resp.Body.Close()

	reader := bufio.NewReader(resp.Body)

	line1, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading line 1: %v", err)
	}
	if line1 != "event: connected\n" {
		t.Errorf("expected line 1 %q, got %q", "event: connected\n", line1)
	}

	line2, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading line 2: %v", err)
	}
	if line2 != "data: {}\n" {
		t.Errorf("expected line 2 %q, got %q", "data: {}\n", line2)
	}

	line3, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading line 3: %v", err)
	}
	if line3 != "\n" {
		t.Errorf("expected line 3 to be blank separator, got %q", line3)
	}
}

// readSSEEvent reads a complete SSE frame (event line + data line + blank
// separator) from reader. It returns the parsed event name and data payload.
// All three lines of the frame are always consumed to keep the parser in sync.
func readSSEEvent(t *testing.T, reader *bufio.Reader) (eventName string, evt watcher.Event) {
	t.Helper()
	line1, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading event line: %v", err)
	}
	eventName = strings.TrimSuffix(strings.TrimPrefix(line1, "event: "), "\n")

	line2, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading data line: %v", err)
	}
	dataStr := strings.TrimSuffix(strings.TrimPrefix(line2, "data: "), "\n")
	if err := json.Unmarshal([]byte(dataStr), &evt); err != nil {
		t.Fatalf("parsing event JSON %q: %v", dataStr, err)
	}

	// consume trailing blank separator
	if _, err := reader.ReadString('\n'); err != nil {
		t.Fatalf("reading separator: %v", err)
	}
	return eventName, evt
}

// E-PENPAL-SSE: verifies broadcast events are delivered to SSE clients as change events.
func TestSSE_BroadcastDelivery(t *testing.T) {
	s, _, _ := testServer(t)
	ts := httptest.NewServer(s)
	defer ts.Close()

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(ts.URL + "/events")
	if err != nil {
		t.Fatalf("GET /events: %v", err)
	}
	defer resp.Body.Close()

	reader := bufio.NewReader(resp.Body)

	// Skip the initial connected event (3 lines)
	for i := 0; i < 3; i++ {
		if _, err := reader.ReadString('\n'); err != nil {
			t.Fatalf("skipping connected event line %d: %v", i+1, err)
		}
	}

	// Broadcast a change event
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged, Project: "test-proj"})

	// Read SSE events until we find our specific broadcast. Background
	// goroutines (populateProjects) may emit events that race with ours.
	var found bool
	for attempts := 0; attempts < 10; attempts++ {
		name, evt := readSSEEvent(t, reader)
		if name != "change" {
			continue
		}
		if evt.Type == "files" && evt.Project == "test-proj" {
			found = true
			break
		}
	}
	if !found {
		t.Error("did not receive expected files event for test-proj")
	}
}

// E-PENPAL-SSE: verifies multiple broadcast events are delivered in order to SSE clients.
func TestSSE_MultipleEvents(t *testing.T) {
	s, _, _ := testServer(t)
	ts := httptest.NewServer(s)
	defer ts.Close()

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(ts.URL + "/events")
	if err != nil {
		t.Fatalf("GET /events: %v", err)
	}
	defer resp.Body.Close()

	reader := bufio.NewReader(resp.Body)

	// Skip the initial connected event (3 lines)
	for i := 0; i < 3; i++ {
		if _, err := reader.ReadString('\n'); err != nil {
			t.Fatalf("skipping connected event line %d: %v", i+1, err)
		}
	}

	// Broadcast 3 different events
	events := []watcher.Event{
		{Type: watcher.EventFilesChanged, Project: "proj-files"},
		{Type: watcher.EventCommentsChanged, Project: "proj-comments"},
		{Type: watcher.EventAgentsChanged, Project: "proj-agents"},
	}
	for _, evt := range events {
		s.watcher.Broadcast(evt)
	}

	// Collect events in order, skipping background noise (e.g. populateProjects).
	// Cap iterations to avoid hanging if an event is dropped.
	var matched []watcher.Event
	nextIdx := 0
	for attempts := 0; attempts < 20 && nextIdx < len(events); attempts++ {
		name, got := readSSEEvent(t, reader)
		if name != "change" {
			continue
		}
		want := events[nextIdx]
		if got.Project != want.Project {
			continue // not one of ours (or out-of-band), skip
		}
		if got.Type != want.Type {
			t.Errorf("event %d: expected type %q, got %q", nextIdx, want.Type, got.Type)
		}
		matched = append(matched, got)
		nextIdx++
	}
	if nextIdx < len(events) {
		t.Fatalf("only matched %d/%d events after 20 iterations", nextIdx, len(events))
	}
}

// E-PENPAL-SSE: verifies handler exits cleanly when client disconnects.
func TestSSE_ClientDisconnect(t *testing.T) {
	s, _, _ := testServer(t)
	ts := httptest.NewServer(s)
	defer ts.Close()

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(ts.URL + "/events")
	if err != nil {
		t.Fatalf("GET /events: %v", err)
	}

	reader := bufio.NewReader(resp.Body)

	// Skip the initial connected event (3 lines)
	for i := 0; i < 3; i++ {
		if _, err := reader.ReadString('\n'); err != nil {
			t.Fatalf("skipping connected event line %d: %v", i+1, err)
		}
	}

	// Close the response body to simulate client disconnect
	resp.Body.Close()

	// Give the server a moment to detect the disconnect
	time.Sleep(50 * time.Millisecond)

	// Broadcasting after disconnect should not panic or error
	s.watcher.Broadcast(watcher.Event{Type: watcher.EventFilesChanged, Project: "ghost"})

	// If we reach here without a panic, the test passes.
	// The handler should have exited cleanly via r.Context().Done().
}
