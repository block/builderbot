package agents

import (
	"bufio"
	"encoding/json"
	"io"
	"os"
)

// streamMessage is a minimal struct for parsing NDJSON from Claude Code's stream-json output.
type streamMessage struct {
	Type    string `json:"type"`
	Message *struct {
		Usage *streamUsage `json:"usage"`
	} `json:"message"`
	TotalCostUSD *float64              `json:"total_cost_usd"`
	ModelUsage   map[string]*modelInfo `json:"modelUsage"`
	NumTurns     *int                  `json:"num_turns"`
}

type streamUsage struct {
	InputTokens              int `json:"input_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
	OutputTokens             int `json:"output_tokens"`
}

type modelInfo struct {
	ContextWindow int `json:"contextWindow"`
}

// parseStream reads NDJSON from the agent's stdout, extracts usage data,
// and writes all output through to the log file.
func (a *Agent) parseStream(r io.Reader, logFile *os.File) {
	scanner := bufio.NewScanner(r)
	scanner.Buffer(make([]byte, 1024*1024), 1024*1024) // 1MB buffer for large messages
	for scanner.Scan() {
		line := scanner.Bytes()
		logFile.Write(line)
		logFile.Write([]byte("\n"))
		a.processLine(line)
	}
}

// processLine parses a single NDJSON line and updates usage fields on the Agent.
func (a *Agent) processLine(line []byte) {
	var msg streamMessage
	if err := json.Unmarshal(line, &msg); err != nil {
		return // ignore non-JSON lines
	}

	switch msg.Type {
	case "assistant":
		if msg.Message != nil && msg.Message.Usage != nil {
			u := msg.Message.Usage
			contextUsed := u.InputTokens + u.CacheReadInputTokens + u.CacheCreationInputTokens
			a.mu.Lock()
			a.contextUsed = contextUsed
			a.numTurns++
			a.mu.Unlock()
		}

	case "result":
		a.mu.Lock()
		if msg.TotalCostUSD != nil {
			a.totalCostUSD = *msg.TotalCostUSD
		}
		if msg.NumTurns != nil {
			a.numTurns = *msg.NumTurns
		}
		for _, info := range msg.ModelUsage {
			if info.ContextWindow > 0 {
				a.contextWindow = info.ContextWindow
				break
			}
		}
		a.mu.Unlock()
	}
}
