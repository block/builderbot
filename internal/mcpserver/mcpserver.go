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
		Instructions: "Birdseye only operates on markdown files inside thoughts/ directories (research, plans, guides). It is NOT for code review. Use these tools to participate in collaborative document review with humans.",
	})
	registerTools(server, store, c)
	return mcp.NewStreamableHTTPHandler(func(r *http.Request) *mcp.Server {
		return server
	}, nil)
}
