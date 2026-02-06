<!--
  GitTreeAnimation.svelte - Generative git tree animation
  
  Shows a git tree growing organically one commit at a time,
  scrolling left as it grows. Idealized git behavior:
  branches spawn from parent branches and merge back to them.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  let canvas: HTMLCanvasElement | null = $state(null);
  let animationId: number | null = null;

  // Animation configuration
  const CONFIG = {
    // Timing
    commitInterval: 600, // ms between new commits

    // Sizing
    circleRadius: 6,
    lineWidth: 2,
    laneSpacing: 28,
    commitSpacing: 44,

    // Tree behavior
    maxDepth: 4, // maximum branch nesting depth (0 = main only)
    branchProbability: 0.2, // chance to create a new branch
    branchUpProbability: 0.25, // when branching, chance to go up instead of down
    minBranchLength: 2, // minimum commits before a branch can merge
    maxBranchLength: 6, // force merge after this many commits
    workOnBranchProbability: 0.6, // prefer working on branches over main
  };

  // Scroll speed: pixels per millisecond to match commit rate
  const SCROLL_SPEED_PER_MS = CONFIG.commitSpacing / CONFIG.commitInterval;

  // Color from theme
  let strokeColor = 'rgba(128, 128, 128, 0.6)';
  let bgColor = '#1a1a1a';

  interface Commit {
    id: number;
    lane: number;
    x: number; // absolute x position
    appearProgress: number; // 0 to 1, for fade-in
    // Primary parent (same lane, or parent lane for merges)
    parentId: number | null;
    // Secondary parent for merges (the branch being merged in)
    mergeParentId: number | null;
    mergeParentLane: number | null; // stored separately in case commit is pruned
    // For branch commits: where we branched from
    branchFromId: number | null;
    branchFromLane: number | null;
    // Frozen positions for pruned parents
    frozenParentX?: number;
    frozenMergeParentX?: number;
    frozenBranchFromX?: number;
  }

  interface Branch {
    lane: number;
    active: boolean;
    headCommitId: number | null;
    commitCount: number; // commits since branch started
    parentBranchLane: number; // which branch this came from
    depth: number; // nesting level (main = 0)
  }

  // State
  let commits: Commit[] = [];
  let branches: Branch[] = [];
  let nextCommitId = 0;
  let nextCommitX = 0;
  let scrollOffset = 0;
  let lastCommitTime = 0;
  let lastFrameTime = 0;
  let scrolling = false;
  let canvasWidth = 0;
  let canvasHeight = 0;

  function updateColors() {
    if (typeof window === 'undefined') return;
    const style = getComputedStyle(document.documentElement);
    strokeColor = style.getPropertyValue('--text-muted').trim() || '#6b7280';
    bgColor = style.getPropertyValue('--bg-primary').trim() || '#1a1a1a';
  }

  function initializeBranches() {
    // Start with just main (lane 0)
    branches = [
      {
        lane: 0,
        active: true,
        headCommitId: null,
        commitCount: 0,
        parentBranchLane: -1, // main has no parent
        depth: 0,
      },
    ];
  }

  function getActiveBranches(): Branch[] {
    return branches.filter((b) => b.active);
  }

  function getBranchByLane(lane: number): Branch | undefined {
    return branches.find((b) => b.lane === lane);
  }

  function getLaneY(lane: number, height: number): number {
    // Center the lanes vertically, with lane 0 at top
    const maxLanes = CONFIG.maxDepth + 1;
    const totalHeight = (maxLanes - 1) * CONFIG.laneSpacing;
    const centerY = height / 2;
    return centerY - totalHeight / 2 + lane * CONFIG.laneSpacing;
  }

  function getCommitById(id: number | null): Commit | undefined {
    if (id === null) return undefined;
    return commits.find((c) => c.id === id);
  }

  function findAvailableLane(parentLane: number): number | null {
    // Find an available lane for a new branch
    // Stack discipline: only branch to adjacent lanes, prefer going down
    const usedLanes = new Set(branches.filter((b) => b.active).map((b) => b.lane));

    // Usually go down (next lane below parent)
    const laneBelow = parentLane + 1;
    if (laneBelow <= CONFIG.maxDepth && !usedLanes.has(laneBelow)) {
      // Occasionally go up instead for visual interest
      const laneAbove = parentLane - 1;
      if (
        laneAbove >= 0 &&
        !usedLanes.has(laneAbove) &&
        Math.random() < CONFIG.branchUpProbability
      ) {
        return laneAbove;
      }
      return laneBelow;
    }

    // If can't go down, try going up one lane
    const laneAbove = parentLane - 1;
    if (laneAbove >= 0 && !usedLanes.has(laneAbove)) {
      return laneAbove;
    }

    return null;
  }

  function canBranchFrom(branch: Branch): boolean {
    // A branch can only spawn a new branch if it's a leaf (has no active children)
    const activeBranches = getActiveBranches();
    const hasActiveChildren = activeBranches.some(
      (other) => other.parentBranchLane === branch.lane
    );
    return !hasActiveChildren && branch.depth < CONFIG.maxDepth;
  }

  function canMerge(branch: Branch): boolean {
    // A branch can only merge if it's a leaf (has no active children)
    if (branch.depth === 0) return false; // main can't merge
    const activeBranches = getActiveBranches();
    const hasActiveChildren = activeBranches.some(
      (other) => other.parentBranchLane === branch.lane
    );
    return !hasActiveChildren;
  }

  function addCommit(
    lane: number,
    parentId: number | null,
    options: {
      mergeParentId?: number | null;
      branchFromId?: number | null;
      branchFromLane?: number | null;
    } = {}
  ): Commit {
    // Look up merge parent lane before creating commit
    let mergeParentLane: number | null = null;
    if (options.mergeParentId != null) {
      const mergeParent = getCommitById(options.mergeParentId);
      if (mergeParent) {
        mergeParentLane = mergeParent.lane;
      }
    }

    const commit: Commit = {
      id: nextCommitId++,
      lane,
      x: nextCommitX,
      appearProgress: 0,
      parentId,
      mergeParentId: options.mergeParentId ?? null,
      mergeParentLane,
      branchFromId: options.branchFromId ?? null,
      branchFromLane: options.branchFromLane ?? null,
    };
    commits.push(commit);

    const branch = getBranchByLane(lane);
    if (branch) {
      branch.headCommitId = commit.id;
      branch.commitCount++;
    }

    return commit;
  }

  function createBranch(sourceBranch: Branch): boolean {
    const newLane = findAvailableLane(sourceBranch.lane);
    if (newLane === null) return false;

    const sourceHead = sourceBranch.headCommitId;
    if (sourceHead === null) return false;

    // Create or reactivate branch at this lane
    let branch = getBranchByLane(newLane);
    if (!branch) {
      branch = {
        lane: newLane,
        active: true,
        headCommitId: null,
        commitCount: 0,
        parentBranchLane: sourceBranch.lane,
        depth: sourceBranch.depth + 1,
      };
      branches.push(branch);
    } else {
      branch.active = true;
      branch.commitCount = 0;
      branch.parentBranchLane = sourceBranch.lane;
      branch.depth = sourceBranch.depth + 1;
    }

    // First commit on new branch - connects back to source
    addCommit(newLane, null, {
      branchFromId: sourceHead,
      branchFromLane: sourceBranch.lane,
    });

    return true;
  }

  function mergeBranch(branch: Branch): boolean {
    const parentBranch = getBranchByLane(branch.parentBranchLane);
    if (!parentBranch || !parentBranch.active) return false;

    const branchHead = branch.headCommitId;
    const parentHead = parentBranch.headCommitId;
    if (branchHead === null) return false;

    // Create merge commit on parent branch
    // It has two parents: the parent branch's head AND the merging branch's head
    addCommit(parentBranch.lane, parentHead, {
      mergeParentId: branchHead,
    });

    // Deactivate the merged branch
    branch.active = false;
    branch.headCommitId = null;
    branch.commitCount = 0;

    return true;
  }

  function generateNextCommit() {
    const activeBranches = getActiveBranches();

    // Find all leaf branches (branches that can merge or branch)
    const leafBranches = activeBranches.filter((b) => canMerge(b) || canBranchFrom(b));

    // Check for branches that must merge (hit max length)
    for (const branch of leafBranches) {
      if (canMerge(branch) && branch.commitCount >= CONFIG.maxBranchLength) {
        if (mergeBranch(branch)) {
          nextCommitX += CONFIG.commitSpacing;
          return;
        }
      }
    }

    // Check for branches ready to merge (probabilistic)
    for (const branch of leafBranches) {
      if (canMerge(branch) && branch.commitCount >= CONFIG.minBranchLength) {
        // Higher merge probability as branch gets longer
        const lengthFactor =
          (branch.commitCount - CONFIG.minBranchLength) /
          (CONFIG.maxBranchLength - CONFIG.minBranchLength);
        const mergeProb = 0.15 + lengthFactor * 0.4; // 15% to 55%

        if (Math.random() < mergeProb) {
          if (mergeBranch(branch)) {
            nextCommitX += CONFIG.commitSpacing;
            return;
          }
        }
      }
    }

    // Maybe create a new branch (from any leaf branch that can branch)
    const branchableBranches = leafBranches.filter((b) => canBranchFrom(b));
    if (branchableBranches.length > 0 && Math.random() < CONFIG.branchProbability) {
      const sourceBranch =
        branchableBranches[Math.floor(Math.random() * branchableBranches.length)];
      if (createBranch(sourceBranch)) {
        nextCommitX += CONFIG.commitSpacing;
        return;
      }
    }

    // Regular commit on an active branch
    // Any active branch can receive commits - pick one randomly with bias toward deeper branches
    const targetBranch = pickRandomBranch(activeBranches);

    addCommit(targetBranch.lane, targetBranch.headCommitId);
    nextCommitX += CONFIG.commitSpacing;
  }

  function pickRandomBranch(activeBranches: Branch[]): Branch {
    // Weight branches by depth - deeper branches are more likely to get commits
    // This simulates "working on feature branches" while still allowing parent branches to progress
    const weights = activeBranches.map((b) => 1 + b.depth * CONFIG.workOnBranchProbability);
    const totalWeight = weights.reduce((a, b) => a + b, 0);

    let random = Math.random() * totalWeight;
    for (let i = 0; i < activeBranches.length; i++) {
      random -= weights[i];
      if (random <= 0) {
        return activeBranches[i];
      }
    }
    return activeBranches[0];
  }

  function pruneOldCommits() {
    // Before pruning, freeze parent positions for any commits that will lose their parent
    const cutoff = scrollOffset - CONFIG.circleRadius;

    for (const commit of commits) {
      // Check if parent is about to be pruned
      if (commit.parentId !== null && commit.frozenParentX === undefined) {
        const parent = getCommitById(commit.parentId);
        if (parent && parent.x <= cutoff) {
          commit.frozenParentX = parent.x;
        }
      }

      // Check if merge parent is about to be pruned
      if (commit.mergeParentId !== null && commit.frozenMergeParentX === undefined) {
        const mergeParent = getCommitById(commit.mergeParentId);
        if (mergeParent && mergeParent.x <= cutoff) {
          commit.frozenMergeParentX = mergeParent.x;
        }
      }

      // Check if branch source is about to be pruned
      if (commit.branchFromId !== null && commit.frozenBranchFromX === undefined) {
        const branchFrom = getCommitById(commit.branchFromId);
        if (branchFrom && branchFrom.x <= cutoff) {
          commit.frozenBranchFromX = branchFrom.x;
        }
      }
    }

    // Now prune
    commits = commits.filter((c) => c.x > cutoff);
  }

  function drawCurve(
    ctx: CanvasRenderingContext2D,
    fromX: number,
    fromY: number,
    toX: number,
    toY: number,
    progress: number
  ) {
    if (progress <= 0) return;

    ctx.beginPath();
    ctx.moveTo(fromX, fromY);

    if (fromY === toY) {
      // Same lane - straight line
      const endX = fromX + (toX - fromX) * progress;
      ctx.lineTo(endX, toY);
    } else {
      // Different lanes - bezier curve
      const midX = fromX + (toX - fromX) / 2;
      const endX = fromX + (toX - fromX) * progress;
      const endY = fromY + (toY - fromY) * progress;
      ctx.bezierCurveTo(midX, fromY, midX, endY, endX, endY);
    }

    ctx.stroke();
  }

  function draw(ctx: CanvasRenderingContext2D, width: number, height: number) {
    ctx.clearRect(0, 0, width, height);

    // Update appearance animations
    for (const commit of commits) {
      if (commit.appearProgress < 1) {
        commit.appearProgress = Math.min(1, commit.appearProgress + 0.08);
      }
    }

    // Draw connections first (behind circles)
    ctx.strokeStyle = strokeColor;
    ctx.lineWidth = CONFIG.lineWidth;
    ctx.lineCap = 'round';
    ctx.globalAlpha = 0.5;

    for (const commit of commits) {
      const commitScreenX = commit.x - scrollOffset;
      const commitY = getLaneY(commit.lane, height);

      // Skip if commit is off the right side
      if (commitScreenX > width + CONFIG.commitSpacing) continue;

      const progress = Math.min(1, commit.appearProgress / 0.7);

      // Draw primary parent connection (same lane continuation)
      if (commit.parentId !== null) {
        const parent = getCommitById(commit.parentId);
        let parentScreenX: number;

        if (commit.frozenParentX !== undefined) {
          parentScreenX = commit.frozenParentX - scrollOffset;
        } else if (parent) {
          parentScreenX = parent.x - scrollOffset;
        } else {
          parentScreenX = -CONFIG.commitSpacing;
        }

        const parentY = parent ? getLaneY(parent.lane, height) : commitY;
        drawCurve(ctx, parentScreenX, parentY, commitScreenX, commitY, progress);
      }

      // Draw merge parent connection (branch being merged in)
      if (commit.mergeParentId !== null && commit.mergeParentLane !== null) {
        const mergeParent = getCommitById(commit.mergeParentId);
        let mergeParentScreenX: number;

        if (commit.frozenMergeParentX !== undefined) {
          mergeParentScreenX = commit.frozenMergeParentX - scrollOffset;
        } else if (mergeParent) {
          mergeParentScreenX = mergeParent.x - scrollOffset;
        } else {
          mergeParentScreenX = -CONFIG.commitSpacing;
        }

        const mergeParentY = getLaneY(commit.mergeParentLane, height);
        drawCurve(ctx, mergeParentScreenX, mergeParentY, commitScreenX, commitY, progress);
      }

      // Draw branch-from connection (first commit on a new branch)
      if (commit.branchFromId !== null && commit.branchFromLane !== null) {
        const branchFrom = getCommitById(commit.branchFromId);
        let branchFromScreenX: number;

        if (commit.frozenBranchFromX !== undefined) {
          branchFromScreenX = commit.frozenBranchFromX - scrollOffset;
        } else if (branchFrom) {
          branchFromScreenX = branchFrom.x - scrollOffset;
        } else {
          branchFromScreenX = -CONFIG.commitSpacing;
        }

        const branchFromY = getLaneY(commit.branchFromLane, height);
        drawCurve(ctx, branchFromScreenX, branchFromY, commitScreenX, commitY, progress);
      }
    }

    // Draw circles on top (hollow/outline style)
    ctx.lineWidth = CONFIG.lineWidth;

    for (const commit of commits) {
      const screenX = commit.x - scrollOffset;
      const y = getLaneY(commit.lane, height);

      // Skip if off screen
      if (screenX < -CONFIG.circleRadius || screenX > width + CONFIG.circleRadius) {
        continue;
      }

      if (commit.appearProgress === 0) continue;

      const scale = commit.appearProgress;
      const radius = CONFIG.circleRadius * scale;

      ctx.globalAlpha = commit.appearProgress * 0.8;
      ctx.strokeStyle = strokeColor;
      ctx.fillStyle = bgColor;

      ctx.beginPath();
      ctx.arc(screenX, y, radius, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
    }

    ctx.globalAlpha = 1;
  }

  function startAnimation() {
    if (!canvas) return;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    // Initialize
    commits = [];
    nextCommitId = 0;
    scrollOffset = 0;
    initializeBranches();

    // Start with initial commit positioned on screen
    nextCommitX = CONFIG.commitSpacing;
    addCommit(0, null);
    nextCommitX += CONFIG.commitSpacing;

    lastCommitTime = performance.now();
    lastFrameTime = performance.now();
    scrolling = false;

    function animate(currentTime: number) {
      if (!canvas || !ctx) return;

      // Calculate delta time for frame-rate independent animation
      const deltaTime = currentTime - lastFrameTime;
      lastFrameTime = currentTime;

      // Handle high DPI displays
      const dpr = window.devicePixelRatio || 1;
      const rect = canvas.getBoundingClientRect();

      if (canvas.width !== rect.width * dpr || canvas.height !== rect.height * dpr) {
        canvas.width = rect.width * dpr;
        canvas.height = rect.height * dpr;
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        canvasWidth = rect.width;
        canvasHeight = rect.height;
      }

      // Add new commits periodically
      if (currentTime - lastCommitTime > CONFIG.commitInterval) {
        generateNextCommit();
        lastCommitTime = currentTime;
      }

      // Scroll logic: fill the screen first, then keep newest commit at 75% position
      const rightmostCommitX = nextCommitX - CONFIG.commitSpacing;
      const targetX = canvasWidth * 0.75;
      const targetScrollOffset = Math.max(0, rightmostCommitX - targetX);

      if (targetScrollOffset > 0) {
        if (!scrolling) {
          // Start scrolling once commits have filled the view
          scrolling = true;
          scrollOffset = targetScrollOffset;
        } else {
          // Scroll at constant rate, but never past the target
          // This provides smooth motion without oscillation
          scrollOffset = Math.min(
            scrollOffset + SCROLL_SPEED_PER_MS * deltaTime,
            targetScrollOffset
          );
        }
      }

      // Prune old commits that have scrolled off
      pruneOldCommits();

      draw(ctx, rect.width, rect.height);
      animationId = requestAnimationFrame(animate);
    }

    animationId = requestAnimationFrame(animate);
  }

  function stopAnimation() {
    if (animationId !== null) {
      cancelAnimationFrame(animationId);
      animationId = null;
    }
  }

  onMount(() => {
    updateColors();
    startAnimation();

    // Listen for theme changes
    const observer = new MutationObserver(() => {
      updateColors();
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['class', 'style'],
    });

    return () => {
      observer.disconnect();
    };
  });

  onDestroy(() => {
    stopAnimation();
  });
</script>

<div class="animation-wrapper">
  <canvas bind:this={canvas} class="git-tree-canvas"></canvas>
</div>

<style>
  .animation-wrapper {
    width: 100%;
    height: 140px;
    overflow: hidden;
  }

  .git-tree-canvas {
    width: 100%;
    height: 100%;
  }
</style>
