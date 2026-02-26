package agents

import (
	"os"
	"strings"
	"sync"
	"testing"
)

func newTestAgent() *Agent {
	return &Agent{
		contextWindow: 200000,
	}
}

func TestProcessLine_AssistantMessage(t *testing.T) {
	a := newTestAgent()
	line := []byte(`{"type":"assistant","message":{"usage":{"input_tokens":3,"cache_read_input_tokens":32568,"cache_creation_input_tokens":256,"output_tokens":1}}}`)
	a.processLine(line)

	a.mu.Lock()
	defer a.mu.Unlock()

	if a.contextUsed != 3+32568+256 {
		t.Errorf("contextUsed = %d, want %d", a.contextUsed, 3+32568+256)
	}
	if a.numTurns != 1 {
		t.Errorf("numTurns = %d, want 1", a.numTurns)
	}
}

func TestProcessLine_MultipleTurns(t *testing.T) {
	a := newTestAgent()

	a.processLine([]byte(`{"type":"assistant","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"output_tokens":5}}}`))
	a.processLine([]byte(`{"type":"assistant","message":{"usage":{"input_tokens":20,"cache_read_input_tokens":200,"cache_creation_input_tokens":100,"output_tokens":10}}}`))

	a.mu.Lock()
	defer a.mu.Unlock()

	// contextUsed should reflect the latest turn, not cumulative
	if a.contextUsed != 20+200+100 {
		t.Errorf("contextUsed = %d, want %d", a.contextUsed, 320)
	}
	if a.numTurns != 2 {
		t.Errorf("numTurns = %d, want 2", a.numTurns)
	}
}

func TestProcessLine_ResultMessage(t *testing.T) {
	a := newTestAgent()
	line := []byte(`{"type":"result","total_cost_usd":0.53,"num_turns":7,"modelUsage":{"claude-opus-4-6":{"contextWindow":200000,"maxOutputTokens":32000,"inputTokens":49,"outputTokens":2586,"cacheReadInputTokens":502157,"cacheCreationInputTokens":34709,"costUSD":0.53}}}`)
	a.processLine(line)

	a.mu.Lock()
	defer a.mu.Unlock()

	if a.totalCostUSD != 0.53 {
		t.Errorf("totalCostUSD = %f, want 0.53", a.totalCostUSD)
	}
	if a.numTurns != 7 {
		t.Errorf("numTurns = %d, want 7", a.numTurns)
	}
	if a.contextWindow != 200000 {
		t.Errorf("contextWindow = %d, want 200000", a.contextWindow)
	}
}

func TestProcessLine_MalformedJSON(t *testing.T) {
	a := newTestAgent()
	a.processLine([]byte(`not json at all`))
	a.processLine([]byte(`{"type":"assistant"}`)) // no usage
	a.processLine([]byte(`{}`))                   // no type

	a.mu.Lock()
	defer a.mu.Unlock()

	if a.contextUsed != 0 {
		t.Errorf("contextUsed = %d, want 0", a.contextUsed)
	}
	if a.numTurns != 0 {
		t.Errorf("numTurns = %d, want 0", a.numTurns)
	}
}

func TestProcessLine_IrrelevantTypes(t *testing.T) {
	a := newTestAgent()
	a.processLine([]byte(`{"type":"user","message":{"role":"user"}}`))
	a.processLine([]byte(`{"type":"stream_event","data":"hello"}`))

	a.mu.Lock()
	defer a.mu.Unlock()

	if a.contextUsed != 0 || a.numTurns != 0 {
		t.Error("irrelevant message types should not update state")
	}
}

func TestParseStream(t *testing.T) {
	input := strings.Join([]string{
		`{"type":"assistant","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"output_tokens":5}}}`,
		`{"type":"user","message":{}}`,
		`{"type":"assistant","message":{"usage":{"input_tokens":20,"cache_read_input_tokens":500,"cache_creation_input_tokens":100,"output_tokens":10}}}`,
		`{"type":"result","total_cost_usd":0.25,"num_turns":2,"modelUsage":{"claude-opus-4-6":{"contextWindow":200000}}}`,
	}, "\n")

	a := newTestAgent()
	tmpFile, err := os.CreateTemp("", "stream-test-*.log")
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(tmpFile.Name())
	defer tmpFile.Close()

	a.parseStream(strings.NewReader(input), tmpFile)

	a.mu.Lock()
	defer a.mu.Unlock()

	if a.contextUsed != 20+500+100 {
		t.Errorf("contextUsed = %d, want %d", a.contextUsed, 620)
	}
	if a.totalCostUSD != 0.25 {
		t.Errorf("totalCostUSD = %f, want 0.25", a.totalCostUSD)
	}
	if a.numTurns != 2 {
		t.Errorf("numTurns = %d, want 2 (from result)", a.numTurns)
	}

	// Verify log file received all lines
	tmpFile.Seek(0, 0)
	logContent, _ := os.ReadFile(tmpFile.Name())
	lines := strings.Split(strings.TrimSpace(string(logContent)), "\n")
	if len(lines) != 4 {
		t.Errorf("log file has %d lines, want 4", len(lines))
	}
}

func TestStatusContextPercent(t *testing.T) {
	a := newTestAgent()
	a.mu.Lock()
	a.contextWindow = 200000
	a.contextUsed = 100000
	a.mu.Unlock()

	m := &Manager{
		agents: map[string]*Agent{"test": a},
	}
	a.Project = "test"
	a.done = make(chan struct{})

	status := m.Status("test")
	if status.ContextPercent != 50.0 {
		t.Errorf("ContextPercent = %f, want 50.0", status.ContextPercent)
	}
}

func TestStatusContextPercentZeroWindow(t *testing.T) {
	a := newTestAgent()
	a.contextWindow = 0 // edge case: no context window info yet

	m := &Manager{
		mu:     sync.Mutex{},
		agents: map[string]*Agent{"test": a},
	}
	a.Project = "test"
	a.done = make(chan struct{})

	status := m.Status("test")
	if status.ContextPercent != 0 {
		t.Errorf("ContextPercent = %f, want 0 (zero window)", status.ContextPercent)
	}
}
