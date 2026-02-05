# Default recipe
default: run

# Build the binary
build:
    go build -o birdseye .

# Run the server
run: build
    ./birdseye

# Run with live reload (requires entr)
dev:
    find . -name '*.go' -o -name '*.html' | entr -r just run

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
