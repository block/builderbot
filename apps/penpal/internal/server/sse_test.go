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

	line1, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading event line 1: %v", err)
	}
	if line1 != "event: change\n" {
		t.Errorf("expected %q, got %q", "event: change\n", line1)
	}

	line2, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading event line 2: %v", err)
	}
	dataStr := strings.TrimPrefix(line2, "data: ")
	dataStr = strings.TrimSuffix(dataStr, "\n")
	var evt watcher.Event
	if err := json.Unmarshal([]byte(dataStr), &evt); err != nil {
		t.Fatalf("parsing event JSON %q: %v", dataStr, err)
	}
	if evt.Type != "files" {
		t.Errorf("expected event type %q, got %q", "files", evt.Type)
	}
	if evt.Project != "test-proj" {
		t.Errorf("expected project %q, got %q", "test-proj", evt.Project)
	}

	line3, err := reader.ReadString('\n')
	if err != nil {
		t.Fatalf("reading event line 3: %v", err)
	}
	if line3 != "\n" {
		t.Errorf("expected blank separator, got %q", line3)
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

	// Read and verify all 3 events (3 lines each = 9 lines)
	for i, want := range events {
		line1, err := reader.ReadString('\n')
		if err != nil {
			t.Fatalf("event %d line 1: %v", i, err)
		}
		if line1 != "event: change\n" {
			t.Errorf("event %d: expected %q, got %q", i, "event: change\n", line1)
		}

		line2, err := reader.ReadString('\n')
		if err != nil {
			t.Fatalf("event %d line 2: %v", i, err)
		}
		dataStr := strings.TrimPrefix(line2, "data: ")
		dataStr = strings.TrimSuffix(dataStr, "\n")
		var got watcher.Event
		if err := json.Unmarshal([]byte(dataStr), &got); err != nil {
			t.Fatalf("event %d: parsing JSON %q: %v", i, dataStr, err)
		}
		if got.Type != want.Type {
			t.Errorf("event %d: expected type %q, got %q", i, want.Type, got.Type)
		}
		if got.Project != want.Project {
			t.Errorf("event %d: expected project %q, got %q", i, want.Project, got.Project)
		}

		line3, err := reader.ReadString('\n')
		if err != nil {
			t.Fatalf("event %d line 3: %v", i, err)
		}
		if line3 != "\n" {
			t.Errorf("event %d: expected blank separator, got %q", i, line3)
		}
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
