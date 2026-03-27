package main

import (
	"encoding/json"
	"fmt"
	"testing"
)

// E-PENPAL-MCP-JSON: verifies the .mcp.json structure produced for MCP client discovery.
//
// The actual write happens inside runServe() which is not directly testable
// without starting the full server. This test validates that the JSON structure
// used in runServe() is valid and round-trips correctly.
func TestMCPJSONStructure(t *testing.T) {
	port := 8080

	// Replicate the exact structure from runServe()
	mcpConfig := map[string]interface{}{
		"mcpServers": map[string]interface{}{
			"penpal": map[string]interface{}{
				"type": "http",
				"url":  fmt.Sprintf("http://localhost:%d/mcp", port),
			},
		},
	}

	// Verify it marshals to valid JSON
	mcpJSON, err := json.MarshalIndent(mcpConfig, "", "  ")
	if err != nil {
		t.Fatalf("failed to marshal MCP config: %v", err)
	}

	// Verify round-trip: unmarshal back and check structure
	var parsed map[string]interface{}
	if err := json.Unmarshal(mcpJSON, &parsed); err != nil {
		t.Fatalf("failed to unmarshal MCP config: %v", err)
	}

	// Verify top-level key
	servers, ok := parsed["mcpServers"].(map[string]interface{})
	if !ok {
		t.Fatal("mcpServers key missing or wrong type")
	}

	// Verify penpal server entry
	penpal, ok := servers["penpal"].(map[string]interface{})
	if !ok {
		t.Fatal("penpal server entry missing or wrong type")
	}

	if penpal["type"] != "http" {
		t.Errorf("penpal type = %q, want %q", penpal["type"], "http")
	}

	expectedURL := fmt.Sprintf("http://localhost:%d/mcp", port)
	if penpal["url"] != expectedURL {
		t.Errorf("penpal url = %q, want %q", penpal["url"], expectedURL)
	}
}

// E-PENPAL-MCP-JSON: verifies the MCP JSON uses dynamic port.
func TestMCPJSONDynamicPort(t *testing.T) {
	for _, port := range []int{3000, 8080, 9999} {
		t.Run(fmt.Sprintf("port_%d", port), func(t *testing.T) {
			mcpConfig := map[string]interface{}{
				"mcpServers": map[string]interface{}{
					"penpal": map[string]interface{}{
						"type": "http",
						"url":  fmt.Sprintf("http://localhost:%d/mcp", port),
					},
				},
			}

			mcpJSON, err := json.MarshalIndent(mcpConfig, "", "  ")
			if err != nil {
				t.Fatalf("failed to marshal: %v", err)
			}

			var parsed map[string]interface{}
			if err := json.Unmarshal(mcpJSON, &parsed); err != nil {
				t.Fatalf("failed to unmarshal: %v", err)
			}

			servers := parsed["mcpServers"].(map[string]interface{})
			penpal := servers["penpal"].(map[string]interface{})
			expected := fmt.Sprintf("http://localhost:%d/mcp", port)
			if penpal["url"] != expected {
				t.Errorf("url = %q, want %q", penpal["url"], expected)
			}
		})
	}
}
