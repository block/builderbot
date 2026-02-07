package mcpserver

import (
	"net/http"

	"github.com/loganj/birdseye/internal/cache"
	"github.com/loganj/birdseye/internal/comments"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// NewHandler creates an HTTP handler implementing the MCP Streamable HTTP
// protocol. It exposes comment and review tools so AI agents can interact
// with birdseye programmatically.
func NewHandler(store *comments.Store, c *cache.Cache) http.Handler {
	server := mcp.NewServer(&mcp.Implementation{
		Name:    "birdseye",
		Version: "1.0.0",
	}, &mcp.ServerOptions{
		Instructions: "Birdseye operates on markdown files for collaborative document review with humans. File paths are relative to the project root (e.g., thoughts/plans/foo.md). It is NOT for code review.",
	})
	registerTools(server, store, c)
	return mcp.NewStreamableHTTPHandler(func(r *http.Request) *mcp.Server {
		return server
	}, nil)
}
