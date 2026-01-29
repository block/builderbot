#!/usr/bin/env bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print functions
print_info() {
    echo -e "${BLUE}==>${NC} $1"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}!${NC} $1"
}

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Darwin*)    OS="macos";;
        Linux*)     OS="linux";;
        *)          OS="unknown";;
    esac

    if [ "$OS" = "unknown" ]; then
        print_error "Unsupported operating system. This installer supports macOS and Linux."
        exit 1
    fi

    print_info "Detected OS: $OS"
}

# Check for required commands
check_prerequisites() {
    print_info "Checking prerequisites..."

    local missing_deps=()

    if ! command -v git &> /dev/null; then
        missing_deps+=("git")
    fi

    if [ ${#missing_deps[@]} -ne 0 ]; then
        print_error "Missing required dependencies: ${missing_deps[*]}"
        echo ""
        echo "Please install the missing dependencies and try again:"

        if [ "$OS" = "macos" ]; then
            echo "  brew install ${missing_deps[*]}"
        elif [ "$OS" = "linux" ]; then
            echo "  sudo apt-get install ${missing_deps[*]} (Debian/Ubuntu)"
            echo "  sudo dnf install ${missing_deps[*]} (Fedora)"
        fi
        exit 1
    fi

    print_success "All prerequisites met"
}

# Clone repository
clone_repo() {
    REPO_URL="https://github.com/baxen/staged.git"
    TEMP_DIR=$(mktemp -d)

    print_info "Cloning repository to $TEMP_DIR..."

    if git clone --depth 1 "$REPO_URL" "$TEMP_DIR" > /dev/null 2>&1; then
        print_success "Repository cloned"
        cd "$TEMP_DIR"
    else
        print_error "Failed to clone repository"
        exit 1
    fi
}

# Setup Hermit
setup_hermit() {
    print_info "Setting up Hermit environment..."

    if [ -f "bin/activate-hermit" ]; then
        # Source hermit in a subshell to avoid affecting current shell
        . bin/activate-hermit

        # Set default Rust toolchain if rustup is available
        if command -v rustup &> /dev/null; then
            rustup default stable > /dev/null 2>&1 || true
        fi

        print_success "Hermit environment activated"
    else
        print_error "Hermit activation script not found"
        exit 1
    fi
}

# Install dependencies
install_deps() {
    print_info "Installing dependencies..."

    # Install npm dependencies
    if npm install > /dev/null 2>&1; then
        print_success "npm dependencies installed"
    else
        print_error "Failed to install npm dependencies"
        exit 1
    fi

    # Fetch cargo dependencies
    if (cd src-tauri && cargo fetch > /dev/null 2>&1); then
        print_success "Cargo dependencies fetched"
    else
        print_error "Failed to fetch cargo dependencies"
        exit 1
    fi
}

# Build the application
build_app() {
    print_info "Building application (this may take a few minutes)..."

    if npm run tauri:build > /dev/null 2>&1; then
        print_success "Application built successfully"
    else
        print_error "Build failed"
        exit 1
    fi
}

# Install to system
install_to_system() {
    if [ "$OS" = "macos" ]; then
        APP_PATH="src-tauri/target/release/bundle/macos/staged.app"
        INSTALL_PATH="/Applications/staged.app"

        print_info "Installing to $INSTALL_PATH..."

        if [ -d "$INSTALL_PATH" ]; then
            print_warning "Existing installation found. Removing..."
            rm -rf "$INSTALL_PATH"
        fi

        if cp -R "$APP_PATH" "$INSTALL_PATH"; then
            print_success "Installed to $INSTALL_PATH"
        else
            print_error "Failed to install application"
            exit 1
        fi
    elif [ "$OS" = "linux" ]; then
        # For Linux, we'd typically install to /usr/local/bin or ~/.local/bin
        print_info "Linux installation not yet implemented"
        print_info "You can find the built binary in: src-tauri/target/release/staged"
    fi
}

# Install CLI command
install_cli() {
    if [ "$OS" = "macos" ]; then
        CLI_PATH="bin/staged"
        INSTALL_PATH="/usr/local/bin/staged"

        print_info "Installing CLI to $INSTALL_PATH..."

        # Create /usr/local/bin if it doesn't exist
        if [ ! -d "/usr/local/bin" ]; then
            sudo mkdir -p /usr/local/bin
        fi

        if sudo cp "$CLI_PATH" "$INSTALL_PATH" && sudo chmod +x "$INSTALL_PATH"; then
            print_success "CLI installed to $INSTALL_PATH"
        else
            print_warning "Failed to install CLI (you can manually copy bin/staged to your PATH)"
        fi
    fi
}

# Cleanup
cleanup() {
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        print_info "Cleaning up temporary files..."
        cd /
        rm -rf "$TEMP_DIR"
        print_success "Cleanup complete"
    fi
}

# Main installation flow
main() {
    echo ""
    echo "╔═══════════════════════════════════════╗"
    echo "║   Staged - Git Diff Viewer Installer  ║"
    echo "╚═══════════════════════════════════════╝"
    echo ""

    detect_os
    check_prerequisites
    clone_repo
    setup_hermit
    install_deps
    build_app
    install_to_system
    install_cli
    cleanup

    echo ""
    print_success "Installation complete!"
    echo ""

    if [ "$OS" = "macos" ]; then
        echo "You can now launch Staged from your Applications folder,"
        echo "or from the command line:"
        echo "  staged          # opens in current directory"
        echo "  staged /path    # opens in specified directory"
    fi

    echo ""
}

# Trap errors and cleanup
trap cleanup EXIT

# Run main installation
main
