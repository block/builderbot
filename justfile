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

    # Wait for server to be ready
    echo "Waiting for server..."
    until curl -s http://localhost:8080/ > /dev/null 2>&1; do
        sleep 0.2
    done
    open "http://localhost:8080"

    wait $PID

# Development mode with hot reload
dev:
    #!/usr/bin/env bash
    # Install fswatch if needed
    if ! command -v fswatch &> /dev/null; then
        echo "Installing fswatch..."
        brew install fswatch
    fi

    cleanup() {
        echo "Stopping server..."
        kill $PID 2>/dev/null
        exit 0
    }
    trap cleanup INT TERM

    go build -o birdseye . && ./birdseye &
    PID=$!

    # Wait for server to be ready before opening browser
    echo "Waiting for server..."
    until curl -s http://localhost:8080/ > /dev/null 2>&1; do
        sleep 0.2
    done
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
