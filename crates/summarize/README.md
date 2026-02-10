# summarize

Portable CLI tool that reads files into an LLM context window with a prompt and returns a summary.

```
summarize --prompt "What are the data models?" src/*.rs
summarize --prompt-file prompt.txt src/a.rs src/b/
```

## Install

```bash
cargo install --path .
```

Or build a static binary:

```bash
cargo build --release
cp target/release/summarize /usr/local/bin/
```

## Auth

Two methods, resolved from environment variables (no config files):

### API Token (simplest)
```bash
export DATABRICKS_HOST="https://your-workspace.databricks.com"
export DATABRICKS_TOKEN="dapi..."
```

### OAuth (browser-based, with token caching + refresh)
```bash
export DATABRICKS_HOST="https://your-workspace.databricks.com"
# No token → triggers OAuth PKCE flow in browser
```

Tokens are cached at `~/.config/summarize/oauth/` and automatically refreshed.

## Model

Default model: `databricks-claude-sonnet-4`

Override with `--model` or `SUMMARIZE_MODEL`:
```bash
summarize --model databricks-meta-llama-3-70b --prompt "Summarize" src/
SUMMARIZE_MODEL=databricks-claude-sonnet-4 summarize --prompt "Explain" lib.rs
```

## Usage

```
summarize [OPTIONS] <PATHS>...

Arguments:
  <PATHS>...   Files or directories (directories expanded recursively)

Options:
  -p, --prompt <PROMPT>          Inline prompt text
  -f, --prompt-file <FILE>       Path to file containing prompt
  -e, --ext <EXT>                Filter by extension (e.g. --ext rs --ext py)
      --model <MODEL>            LLM model name
      --output <text|json>       Output format (default: text)
  -h, --help                     Print help
  -V, --version                  Print version
```

### Examples

```bash
# Summarize Rust source files
summarize --prompt "What are the main abstractions?" src/

# Filter to only Python files
summarize --prompt "List all classes" . --ext py

# JSON output for scripting
summarize --prompt "Find bugs" src/ --output json

# Prompt from file
echo "What security issues exist?" > prompt.txt
summarize --prompt-file prompt.txt src/auth.rs
```

## Features

- **Deterministic** — files are sorted and read in consistent order
- **Gitignore-aware** — respects `.gitignore` rules, always skips `.git/`
- **Binary-safe** — silently skips files that aren't valid UTF-8
- **Extension filter** — `--ext rs --ext py` to limit file types
- **JSON output** — `--output json` for machine-readable results
- **OAuth caching** — browser login only needed once per workspace
- **Zero config files** — everything via env vars or CLI flags
