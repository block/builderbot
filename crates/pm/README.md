# pm

Project manager for multi-repo workspaces. Organizes repos into **projects** using git worktrees and a shared pool, so you can work on multiple features across multiple repos without cloning everything N times.

## How it works

```
workspace/
├── .pm/
│   ├── state.json          # tracks projects, repos, pool slots
│   ├── repos/              # bare clones
│   │   └── myrepo.git
│   └── pool/               # worktrees
│       ├── myrepo--0       # slot 0
│       └── myrepo--1       # slot 1
├── feature-a/
│   └── myrepo -> .pm/pool/myrepo--0   # symlink
└── feature-b/
    └── myrepo -> .pm/pool/myrepo--1   # symlink
```

Each repo gets a fixed number of **pool slots** (default: 2). When you `pm add` a repo to a project, pm acquires a slot, checks out a branch, and symlinks it into your project directory. When all slots are taken, you choose to evict another project or grow the pool.

## Install

```sh
cargo install --path crates/pm
```

## Usage

### Create a project

```sh
pm new my-feature
cd my-feature
```

### Add repos

```sh
# shorthand — clones from GitHub
pm add org/repo

# full URL
pm add git@github.com:org/repo.git

# explicit branch (default: $USER/$PROJECT)
pm add org/repo --branch my-branch

# from a different directory
pm add org/repo --project my-feature
```

### Use an existing checkout (skip the clone)

For heavy repos you've already cloned:

```sh
# symlink directly — pm won't manage branches
pm add myrepo --existing ~/src/myrepo

# worktree mode — pm creates per-project branches from your checkout
pm add myrepo --existing ~/src/myrepo --worktree
```

### Handle pool conflicts

When all slots are full, pm prompts interactively. In scripts/CI, use flags:

```sh
pm add org/repo --evict stale-project
pm add org/repo --grow-pool
```

### Check status

```sh
pm status
```

Shows all projects, their repos/branches, and pool slot usage.

### Clean up

```sh
pm cleanup                # default: flag projects inactive >14 days
pm cleanup --stale-days 7
```

Finds stale projects, missing directories, and merged branches. Suggests `pm rm <project>` for each.

### Remove a project

```sh
pm rm my-feature
```

Removes the directory, releases pool slots, and cleans up state. Also handles projects that were manually `rm -rf`'d.

## Workspace detection

pm walks up from your cwd looking for `.pm/state.json`. A new workspace is auto-initialized if none exists. Override with `--root <path>`.
