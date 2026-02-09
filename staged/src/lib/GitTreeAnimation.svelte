<!--
  GitTreeAnimation.svelte - Generative git tree animation

  Shows a git tree growing organically one commit at a time,
  scrolling left as it grows. Branches spawn from parent branches
  and merge back to them, simulating idealized git behavior.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  let canvas: HTMLCanvasElement | null = $state(null);
  let animationId: number | null = null;

  const CONFIG = {
    commitInterval: 600,
    circleRadius: 6,
    lineWidth: 2,
    laneSpacing: 28,
    commitSpacing: 44,
    maxDepth: 4,
    branchProbability: 0.2,
    branchUpProbability: 0.25,
    minBranchLength: 2,
    maxBranchLength: 6,
    workOnBranchProbability: 0.6,
  };

  const SCROLL_SPEED_PER_MS = CONFIG.commitSpacing / CONFIG.commitInterval;

  let strokeColor = 'rgba(128, 128, 128, 0.6)';
  let bgColor = '#1a1a1a';

  interface Commit {
    id: number;
    lane: number;
    x: number;
    appearProgress: number;
    parentId: number | null;
    mergeParentId: number | null;
    mergeParentLane: number | null;
    branchFromId: number | null;
    branchFromLane: number | null;
    frozenParentX?: number;
    frozenMergeParentX?: number;
    frozenBranchFromX?: number;
  }

  interface Branch {
    lane: number;
    active: boolean;
    headCommitId: number | null;
    commitCount: number;
    parentBranchLane: number;
    depth: number;
  }

  let commits: Commit[] = [];
  let branches: Branch[] = [];
  let nextCommitId = 0;
  let nextCommitX = 0;
  let scrollOffset = 0;
  let lastCommitTime = 0;
  let lastFrameTime = 0;
  let scrolling = false;
  let canvasWidth = 0;

  function updateColors() {
    if (typeof window === 'undefined') return;
    const style = getComputedStyle(document.documentElement);
    strokeColor = style.getPropertyValue('--text-muted').trim() || '#6b7280';
    bgColor = style.getPropertyValue('--bg-chrome').trim() || '#1a1a1a';
  }

  function getActiveBranches(): Branch[] {
    return branches.filter((b) => b.active);
  }

  function getBranchByLane(lane: number): Branch | undefined {
    return branches.find((b) => b.lane === lane);
  }

  function getLaneY(lane: number, height: number): number {
    const totalHeight = CONFIG.maxDepth * CONFIG.laneSpacing;
    return (height - totalHeight) / 2 + lane * CONFIG.laneSpacing;
  }

  function getCommitById(id: number | null): Commit | undefined {
    if (id === null) return undefined;
    return commits.find((c) => c.id === id);
  }

  function findAvailableLane(parentLane: number): number | null {
    const usedLanes = new Set(getActiveBranches().map((b) => b.lane));

    const laneBelow = parentLane + 1;
    if (laneBelow <= CONFIG.maxDepth && !usedLanes.has(laneBelow)) {
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

    const laneAbove = parentLane - 1;
    if (laneAbove >= 0 && !usedLanes.has(laneAbove)) return laneAbove;

    return null;
  }

  function canBranchFrom(branch: Branch): boolean {
    return (
      !getActiveBranches().some((b) => b.parentBranchLane === branch.lane) &&
      branch.depth < CONFIG.maxDepth
    );
  }

  function canMerge(branch: Branch): boolean {
    if (branch.depth === 0) return false;
    return !getActiveBranches().some((b) => b.parentBranchLane === branch.lane);
  }

  function addCommit(
    lane: number,
    parentId: number | null,
    opts: {
      mergeParentId?: number | null;
      branchFromId?: number | null;
      branchFromLane?: number | null;
    } = {}
  ): Commit {
    let mergeParentLane: number | null = null;
    if (opts.mergeParentId != null) {
      const mp = getCommitById(opts.mergeParentId);
      if (mp) mergeParentLane = mp.lane;
    }

    const commit: Commit = {
      id: nextCommitId++,
      lane,
      x: nextCommitX,
      appearProgress: 0,
      parentId,
      mergeParentId: opts.mergeParentId ?? null,
      mergeParentLane,
      branchFromId: opts.branchFromId ?? null,
      branchFromLane: opts.branchFromLane ?? null,
    };
    commits.push(commit);

    const branch = getBranchByLane(lane);
    if (branch) {
      branch.headCommitId = commit.id;
      branch.commitCount++;
    }

    return commit;
  }

  function createBranch(source: Branch): boolean {
    const newLane = findAvailableLane(source.lane);
    if (newLane === null || source.headCommitId === null) return false;

    let branch = getBranchByLane(newLane);
    if (!branch) {
      branch = {
        lane: newLane,
        active: true,
        headCommitId: null,
        commitCount: 0,
        parentBranchLane: source.lane,
        depth: source.depth + 1,
      };
      branches.push(branch);
    } else {
      branch.active = true;
      branch.commitCount = 0;
      branch.parentBranchLane = source.lane;
      branch.depth = source.depth + 1;
    }

    addCommit(newLane, null, {
      branchFromId: source.headCommitId,
      branchFromLane: source.lane,
    });

    return true;
  }

  function mergeBranch(branch: Branch): boolean {
    const parent = getBranchByLane(branch.parentBranchLane);
    if (!parent?.active || branch.headCommitId === null) return false;

    addCommit(parent.lane, parent.headCommitId, { mergeParentId: branch.headCommitId });

    branch.active = false;
    branch.headCommitId = null;
    branch.commitCount = 0;
    return true;
  }

  function generateNextCommit() {
    const active = getActiveBranches();
    const leaves = active.filter((b) => canMerge(b) || canBranchFrom(b));

    // Force-merge branches at max length
    for (const b of leaves) {
      if (canMerge(b) && b.commitCount >= CONFIG.maxBranchLength) {
        if (mergeBranch(b)) {
          nextCommitX += CONFIG.commitSpacing;
          return;
        }
      }
    }

    // Probabilistic merge for ready branches
    for (const b of leaves) {
      if (canMerge(b) && b.commitCount >= CONFIG.minBranchLength) {
        const t =
          (b.commitCount - CONFIG.minBranchLength) /
          (CONFIG.maxBranchLength - CONFIG.minBranchLength);
        if (Math.random() < 0.15 + t * 0.4) {
          if (mergeBranch(b)) {
            nextCommitX += CONFIG.commitSpacing;
            return;
          }
        }
      }
    }

    // Maybe start a new branch
    const branchable = leaves.filter((b) => canBranchFrom(b));
    if (branchable.length > 0 && Math.random() < CONFIG.branchProbability) {
      const src = branchable[Math.floor(Math.random() * branchable.length)];
      if (createBranch(src)) {
        nextCommitX += CONFIG.commitSpacing;
        return;
      }
    }

    // Regular commit on a weighted-random branch
    const weights = active.map((b) => 1 + b.depth * CONFIG.workOnBranchProbability);
    const total = weights.reduce((a, b) => a + b, 0);
    let r = Math.random() * total;
    let target = active[0];
    for (let i = 0; i < active.length; i++) {
      r -= weights[i];
      if (r <= 0) {
        target = active[i];
        break;
      }
    }

    addCommit(target.lane, target.headCommitId);
    nextCommitX += CONFIG.commitSpacing;
  }

  function pruneOldCommits() {
    const cutoff = scrollOffset - CONFIG.circleRadius;

    for (const c of commits) {
      if (c.parentId !== null && c.frozenParentX === undefined) {
        const p = getCommitById(c.parentId);
        if (p && p.x <= cutoff) c.frozenParentX = p.x;
      }
      if (c.mergeParentId !== null && c.frozenMergeParentX === undefined) {
        const mp = getCommitById(c.mergeParentId);
        if (mp && mp.x <= cutoff) c.frozenMergeParentX = mp.x;
      }
      if (c.branchFromId !== null && c.frozenBranchFromX === undefined) {
        const bf = getCommitById(c.branchFromId);
        if (bf && bf.x <= cutoff) c.frozenBranchFromX = bf.x;
      }
    }

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
      ctx.lineTo(fromX + (toX - fromX) * progress, toY);
    } else {
      const midX = fromX + (toX - fromX) / 2;
      const endX = fromX + (toX - fromX) * progress;
      const endY = fromY + (toY - fromY) * progress;
      ctx.bezierCurveTo(midX, fromY, midX, endY, endX, endY);
    }

    ctx.stroke();
  }

  function draw(ctx: CanvasRenderingContext2D, width: number, height: number) {
    ctx.clearRect(0, 0, width, height);

    for (const c of commits) {
      if (c.appearProgress < 1) c.appearProgress = Math.min(1, c.appearProgress + 0.08);
    }

    // Draw connections
    ctx.strokeStyle = strokeColor;
    ctx.lineWidth = CONFIG.lineWidth;
    ctx.lineCap = 'round';
    ctx.globalAlpha = 0.5;

    for (const c of commits) {
      const cx = c.x - scrollOffset;
      const cy = getLaneY(c.lane, height);
      if (cx > width + CONFIG.commitSpacing) continue;

      const progress = Math.min(1, c.appearProgress / 0.7);

      // Parent connection
      if (c.parentId !== null) {
        const parent = getCommitById(c.parentId);
        const px =
          c.frozenParentX !== undefined
            ? c.frozenParentX - scrollOffset
            : parent
              ? parent.x - scrollOffset
              : -CONFIG.commitSpacing;
        const py = parent ? getLaneY(parent.lane, height) : cy;
        drawCurve(ctx, px, py, cx, cy, progress);
      }

      // Merge parent connection
      if (c.mergeParentId !== null && c.mergeParentLane !== null) {
        const mp = getCommitById(c.mergeParentId);
        const mpx =
          c.frozenMergeParentX !== undefined
            ? c.frozenMergeParentX - scrollOffset
            : mp
              ? mp.x - scrollOffset
              : -CONFIG.commitSpacing;
        drawCurve(ctx, mpx, getLaneY(c.mergeParentLane, height), cx, cy, progress);
      }

      // Branch-from connection
      if (c.branchFromId !== null && c.branchFromLane !== null) {
        const bf = getCommitById(c.branchFromId);
        const bfx =
          c.frozenBranchFromX !== undefined
            ? c.frozenBranchFromX - scrollOffset
            : bf
              ? bf.x - scrollOffset
              : -CONFIG.commitSpacing;
        drawCurve(ctx, bfx, getLaneY(c.branchFromLane, height), cx, cy, progress);
      }
    }

    // Draw commit circles
    ctx.lineWidth = CONFIG.lineWidth;

    for (const c of commits) {
      const sx = c.x - scrollOffset;
      const sy = getLaneY(c.lane, height);
      if (sx < -CONFIG.circleRadius || sx > width + CONFIG.circleRadius) continue;
      if (c.appearProgress === 0) continue;

      const radius = CONFIG.circleRadius * c.appearProgress;

      // Fill at full opacity so lines behind are fully covered
      ctx.globalAlpha = 1;
      ctx.fillStyle = bgColor;
      ctx.beginPath();
      ctx.arc(sx, sy, radius, 0, Math.PI * 2);
      ctx.fill();

      // Stroke at reduced opacity for soft appearance
      ctx.globalAlpha = c.appearProgress * 0.8;
      ctx.strokeStyle = strokeColor;
      ctx.stroke();
    }

    ctx.globalAlpha = 1;
  }

  function startAnimation() {
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    commits = [];
    branches = [
      { lane: 0, active: true, headCommitId: null, commitCount: 0, parentBranchLane: -1, depth: 0 },
    ];
    nextCommitId = 0;
    scrollOffset = 0;
    scrolling = false;

    nextCommitX = CONFIG.commitSpacing;
    addCommit(0, null);
    nextCommitX += CONFIG.commitSpacing;

    lastCommitTime = performance.now();
    lastFrameTime = performance.now();

    function animate(now: number) {
      if (!canvas || !ctx) return;

      const dt = now - lastFrameTime;
      lastFrameTime = now;

      // Handle HiDPI
      const dpr = window.devicePixelRatio || 1;
      const rect = canvas.getBoundingClientRect();
      if (canvas.width !== rect.width * dpr || canvas.height !== rect.height * dpr) {
        canvas.width = rect.width * dpr;
        canvas.height = rect.height * dpr;
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        canvasWidth = rect.width;
      }

      if (now - lastCommitTime > CONFIG.commitInterval) {
        generateNextCommit();
        lastCommitTime = now;
      }

      // Scroll to keep newest commit at ~75% across
      const rightmost = nextCommitX - CONFIG.commitSpacing;
      const target = Math.max(0, rightmost - canvasWidth * 0.75);
      if (target > 0) {
        if (!scrolling) {
          scrolling = true;
          scrollOffset = target;
        } else {
          scrollOffset = Math.min(scrollOffset + SCROLL_SPEED_PER_MS * dt, target);
        }
      }

      pruneOldCommits();
      draw(ctx, rect.width, rect.height);
      animationId = requestAnimationFrame(animate);
    }

    animationId = requestAnimationFrame(animate);
  }

  onMount(() => {
    updateColors();
    startAnimation();

    const observer = new MutationObserver(() => updateColors());
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['class', 'style'],
    });

    return () => observer.disconnect();
  });

  onDestroy(() => {
    if (animationId !== null) cancelAnimationFrame(animationId);
  });
</script>

<div class="animation-wrapper">
  <canvas bind:this={canvas}></canvas>
</div>

<style>
  .animation-wrapper {
    width: 100%;
    height: 140px;
    overflow: hidden;
  }

  canvas {
    width: 100%;
    height: 100%;
  }
</style>
