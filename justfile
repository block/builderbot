# Default recipe
default: run

# Build the binary
build:
    go build -o birdseye .

# Run the server and open browser
run: build
    #!/usr/bin/env bash
    ./birdseye &
    PID=$!
    sleep 1
    open "http://localhost:8080"
    wait $PID

# Development mode with hot reload (requires fswatch: brew install fswatch)
dev:
    #!/usr/bin/env bash
    cleanup() {
        echo "Stopping server..."
        kill $PID 2>/dev/null
        exit 0
    }
    trap cleanup INT TERM

    go build -o birdseye . && ./birdseye &
    PID=$!
    sleep 1
    open "http://localhost:8080"

    echo "Watching for changes... (Ctrl+C to stop)"
    fswatch -o -r --include='\.go$' --include='\.html$' --exclude='.*' . | while read; do
        echo "Change detected, rebuilding..."
        kill $PID 2>/dev/null
        go build -o birdseye . && ./birdseye &
        PID=$!
    done

# Clean build artifacts
clean:
    rm -f birdseye

# Format code
fmt:
    go fmt ./...

# Run tests
test:
    go test ./...

# Tidy dependencies
tidy:
    go mod tidy
