/**
 * Generates a complex markdown document with all standard elements in a
 * randomized configuration, plus a manifest of commentable regions.
 *
 * Used by the anchor-stability test to create realistic documents for
 * exercising the highlight anchoring system.
 */

export interface CommentableRegion {
  lineNumber: number; // 1-based start line in the markdown source
  endLineNumber: number; // last line of this block element
  text: string; // substring suitable for selectedText anchoring
  type: 'paragraph' | 'heading' | 'listItem' | 'blockquote';
}

export interface GeneratedDocument {
  markdown: string;
  regions: CommentableRegion[];
}

// Simple seeded PRNG (mulberry32)
function createRng(seed: number) {
  let s = seed | 0;
  return () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function shuffle<T>(arr: T[], rng: () => number): T[] {
  const result = [...arr];
  for (let i = result.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [result[i], result[j]] = [result[j], result[i]];
  }
  return result;
}

interface Block {
  lines: string[];
  regions: Array<{
    offsetFromStart: number; // line offset from block start
    endOffsetFromStart: number;
    text: string;
    type: CommentableRegion['type'];
  }>;
}

function headingBlock(level: number, id: number): Block {
  const prefix = '#'.repeat(level);
  const text = `Section ${id}: ${['Architecture', 'Design Patterns', 'Performance Metrics', 'Security Considerations', 'Deployment Strategy', 'Testing Methodology', 'Data Modeling', 'API Contracts', 'Error Handling', 'Observability'][id % 10]}`;
  return {
    lines: [`${prefix} ${text}`, ''],
    regions: [{ offsetFromStart: 0, endOffsetFromStart: 0, text, type: 'heading' }],
  };
}

function paragraphBlock(id: number): Block {
  const paragraphs: string[] = [
    `The distributed consensus algorithm ensures that all nodes in the cluster agree on the current state before proceeding with transaction ${id}. This prevents split-brain scenarios and maintains data integrity across replicas.`,
    `When evaluating cache eviction strategies for workload ${id}, consider both the temporal locality of access patterns and the cost of recomputation. A two-level cache with an LRU front and LFU back often outperforms either strategy alone.`,
    `Error propagation in asynchronous pipeline ${id} requires careful attention to backpressure signals. Without proper flow control, a slow consumer can cause unbounded memory growth in upstream buffers.`,
    `The schema migration for dataset ${id} introduces a **backwards-compatible** change that adds nullable columns rather than modifying existing ones. This allows rolling deployments without downtime.`,
    `Observability for service ${id} relies on three pillars: structured logging with \`correlation_id\` propagation, distributed tracing via OpenTelemetry, and custom metrics exported to Prometheus.`,
    `Rate limiting for API endpoint ${id} uses a sliding window counter backed by Redis. The algorithm allows brief bursts up to 2x the nominal rate while maintaining the average over any 60-second window.`,
    `The retry policy for integration ${id} follows exponential backoff with jitter: base delay of 100ms, multiplier of 2, maximum delay of 30 seconds, and a randomization factor of 0.25 to prevent thundering herd.`,
    `Feature flag evaluation for experiment ${id} is performed at the edge using a compiled ruleset. This eliminates the latency of a remote flag service call and ensures consistent behavior within a single request.`,
    `Memory allocation profiling for module ${id} revealed that 73% of heap pressure comes from short-lived string concatenations in the serialization path. Switching to a pooled buffer strategy reduced GC pause times by 4x.`,
    `The circuit breaker for dependency ${id} transitions from *closed* to *open* after 5 consecutive failures within a 10-second window. It enters *half-open* after a 30-second cool-down period.`,
    `Database connection pooling for service ${id} maintains a minimum of 10 idle connections and scales up to 50 under load. Connections older than 5 minutes are recycled to avoid stale TCP state.`,
    `The message queue consumer for topic ${id} uses at-least-once delivery semantics with idempotent processing. Each message carries a unique \`deduplication_key\` that is checked against a sliding 24-hour window.`,
  ];
  const text = paragraphs[id % paragraphs.length];
  return {
    lines: [text, ''],
    regions: [{ offsetFromStart: 0, endOffsetFromStart: 0, text: text.replace(/[*`]/g, ''), type: 'paragraph' }],
  };
}

function codeBlock(id: number): Block {
  const blocks = [
    {
      lang: 'go',
      code: [
        `func processBatch${id}(ctx context.Context, items []Item) error {`,
        `    g, ctx := errgroup.WithContext(ctx)`,
        `    for _, item := range items {`,
        `        item := item // capture loop variable`,
        `        g.Go(func() error {`,
        `            return handleItem(ctx, item)`,
        `        })`,
        `    }`,
        `    return g.Wait()`,
        `}`,
      ],
    },
    {
      lang: 'typescript',
      code: [
        `async function fetchWithRetry${id}(url: string, attempts = 3): Promise<Response> {`,
        `  for (let i = 0; i < attempts; i++) {`,
        `    try {`,
        `      const res = await fetch(url);`,
        `      if (res.ok) return res;`,
        `    } catch (err) {`,
        `      if (i === attempts - 1) throw err;`,
        `      await new Promise(r => setTimeout(r, 100 * 2 ** i));`,
        `    }`,
        `  }`,
        `  throw new Error('unreachable');`,
        `}`,
      ],
    },
    {
      lang: 'python',
      code: [
        `def compute_rolling_avg_${id}(values: list[float], window: int) -> list[float]:`,
        `    result = []`,
        `    for i in range(len(values)):`,
        `        start = max(0, i - window + 1)`,
        `        result.append(sum(values[start:i+1]) / (i - start + 1))`,
        `    return result`,
      ],
    },
  ];
  const block = blocks[id % blocks.length];
  return {
    lines: [`\`\`\`${block.lang}`, ...block.code, '```', ''],
    regions: [], // code blocks are not commentable
  };
}

function mermaidBlock(id: number): Block {
  const diagrams = [
    [
      'graph TD',
      `    A${id}[Client Request] --> B${id}{Load Balancer}`,
      `    B${id} --> C${id}[Service A]`,
      `    B${id} --> D${id}[Service B]`,
      `    C${id} --> E${id}[(Database)]`,
      `    D${id} --> E${id}`,
      `    C${id} --> F${id}[Cache Layer]`,
      `    D${id} --> F${id}`,
    ],
    [
      'sequenceDiagram',
      `    participant U${id} as User`,
      `    participant G${id} as Gateway`,
      `    participant S${id} as Service`,
      `    participant D${id} as Database`,
      `    U${id}->>G${id}: HTTP Request`,
      `    G${id}->>S${id}: gRPC Call`,
      `    S${id}->>D${id}: Query`,
      `    D${id}-->>S${id}: Results`,
      `    S${id}-->>G${id}: Response`,
      `    G${id}-->>U${id}: JSON`,
    ],
    [
      'flowchart LR',
      `    A${id}[Input] --> B${id}[Validate]`,
      `    B${id} --> C${id}{Valid?}`,
      `    C${id} -->|Yes| D${id}[Process]`,
      `    C${id} -->|No| E${id}[Reject]`,
      `    D${id} --> F${id}[Store]`,
      `    F${id} --> G${id}[Notify]`,
    ],
  ];
  const diagram = diagrams[id % diagrams.length];
  return {
    lines: ['```mermaid', ...diagram, '```', ''],
    regions: [], // mermaid diagrams are not commentable via text
  };
}

function tableBlock(id: number): Block {
  const tables = [
    {
      header: `| Metric ${id} | Value | Threshold | Status |`,
      sep: '| --- | --- | --- | --- |',
      rows: [
        `| P99 Latency | 45ms | 100ms | Healthy |`,
        `| Error Rate | 0.02% | 1.0% | Healthy |`,
        `| Throughput | 12,400 rps | 10,000 rps | Above Target |`,
        `| CPU Usage | 67% | 80% | Warning |`,
      ],
    },
    {
      header: `| Component ${id} | Owner | SLA | Last Incident |`,
      sep: '| --- | --- | --- | --- |',
      rows: [
        '| Auth Service | Team Alpha | 99.99% | 2024-01-15 |',
        '| Payment Gateway | Team Beta | 99.95% | 2024-02-03 |',
        '| Notification Hub | Team Gamma | 99.9% | 2024-03-21 |',
      ],
    },
  ];
  const table = tables[id % tables.length];
  return {
    lines: [table.header, table.sep, ...table.rows, ''],
    regions: [], // tables are hard to anchor reliably
  };
}

function listBlock(id: number): Block {
  const items = [
    `Implement connection pooling with configurable min/max bounds for service ${id}`,
    `Add circuit breaker with half-open state transition logic for dependency ${id}`,
    `Configure structured logging with request correlation for pipeline ${id}`,
    `Set up distributed tracing spans for cross-service calls in module ${id}`,
  ];
  const regions: Block['regions'] = [];
  const lines: string[] = [];
  items.forEach((item, i) => {
    lines.push(`- ${item}`);
    regions.push({
      offsetFromStart: i,
      endOffsetFromStart: i,
      text: item,
      type: 'listItem',
    });
  });
  lines.push('');
  return { lines, regions };
}

function orderedListBlock(id: number): Block {
  const items = [
    `Initialize the configuration registry for environment ${id}`,
    `Validate all required secrets are present in the vault for deployment ${id}`,
    `Run database migrations with automatic rollback for schema ${id}`,
    `Execute smoke tests against the staging endpoint for release ${id}`,
  ];
  const regions: Block['regions'] = [];
  const lines: string[] = [];
  items.forEach((item, i) => {
    lines.push(`${i + 1}. ${item}`);
    regions.push({
      offsetFromStart: i,
      endOffsetFromStart: i,
      text: item,
      type: 'listItem',
    });
  });
  lines.push('');
  return { lines, regions };
}

function blockquoteBlock(id: number): Block {
  const quotes = [
    `The primary bottleneck in system ${id} is not CPU or memory, but the serialization overhead at service boundaries. Reducing payload size by 40% through selective field projection improved end-to-end latency by 3x.`,
    `After investigating the incident in module ${id}, we determined the root cause was a connection leak triggered by a race condition in the cleanup handler. The fix adds a deferred close with a context timeout.`,
    `Design principle for architecture ${id}: prefer explicit over implicit dependencies. Every service should declare its complete dependency graph in its configuration, enabling automated impact analysis.`,
  ];
  const text = quotes[id % quotes.length];
  return {
    lines: [`> ${text}`, ''],
    regions: [{ offsetFromStart: 0, endOffsetFromStart: 0, text, type: 'blockquote' }],
  };
}

function taskListBlock(id: number): Block {
  const lines = [
    `- [x] Complete design review for feature ${id}`,
    `- [ ] Implement unit tests for edge cases in module ${id}`,
    `- [ ] Update runbook with new operational procedures for service ${id}`,
    `- [x] Verify backwards compatibility for API version ${id}`,
    '',
  ];
  return { lines, regions: [] }; // task lists use checkboxes, skip for anchoring
}

function thematicBreak(): Block {
  return { lines: ['---', ''], regions: [] };
}

export interface GeneratorOptions {
  /** When true, generates a document with repeated/duplicate content blocks
   *  to test anchor stability with ambiguous text (PENPAL-41). */
  repetitive?: boolean;
}

export function generateMarkdownDocument(seed = 42, options?: GeneratorOptions): GeneratedDocument {
  const rng = createRng(seed);
  const repetitive = options?.repetitive ?? false;

  // Build a pool of blocks with unique IDs
  let blockId = 0;
  const blocks: Block[] = [];

  // Always start with a title
  blocks.push(headingBlock(1, blockId++));

  // Build diverse content blocks
  const contentBlocks: Block[] = [];
  if (repetitive) {
    // PENPAL-41: Generate repeated content — same blockId reused so text duplicates
    for (let i = 0; i < 3; i++) contentBlocks.push(headingBlock(2, blockId++));
    // Use the SAME id for multiple paragraphs to create duplicate text
    const sharedParaId = blockId++;
    for (let i = 0; i < 4; i++) contentBlocks.push(paragraphBlock(sharedParaId));
    for (let i = 0; i < 2; i++) contentBlocks.push(paragraphBlock(blockId++));
    // Duplicate list blocks with same id
    const sharedListId = blockId++;
    for (let i = 0; i < 3; i++) contentBlocks.push(listBlock(sharedListId));
    // Duplicate blockquotes with same id
    const sharedQuoteId = blockId++;
    for (let i = 0; i < 2; i++) contentBlocks.push(blockquoteBlock(sharedQuoteId));
    contentBlocks.push(orderedListBlock(blockId++));
  } else {
    for (let i = 0; i < 3; i++) contentBlocks.push(headingBlock(2, blockId++));
    for (let i = 0; i < 2; i++) contentBlocks.push(headingBlock(3, blockId++));
    for (let i = 0; i < 6; i++) contentBlocks.push(paragraphBlock(blockId++));
    for (let i = 0; i < 2; i++) contentBlocks.push(codeBlock(blockId++));
    contentBlocks.push(mermaidBlock(blockId++));
    for (let i = 0; i < 2; i++) contentBlocks.push(tableBlock(blockId++));
    for (let i = 0; i < 2; i++) contentBlocks.push(listBlock(blockId++));
    contentBlocks.push(orderedListBlock(blockId++));
    for (let i = 0; i < 2; i++) contentBlocks.push(blockquoteBlock(blockId++));
    contentBlocks.push(taskListBlock(blockId++));
  }

  // Insert thematic breaks randomly
  const withBreaks = shuffle(contentBlocks, rng);
  const breakPositions = new Set<number>();
  for (let i = 0; i < 3; i++) {
    breakPositions.add(Math.floor(rng() * withBreaks.length));
  }

  const orderedBlocks: Block[] = [blocks[0]]; // title first
  for (let i = 0; i < withBreaks.length; i++) {
    if (breakPositions.has(i)) {
      orderedBlocks.push(thematicBreak());
    }
    orderedBlocks.push(withBreaks[i]);
  }

  // Assemble markdown and compute regions with absolute line numbers
  const allLines: string[] = [];
  const regions: CommentableRegion[] = [];

  for (const block of orderedBlocks) {
    const startLine = allLines.length + 1; // 1-based
    allLines.push(...block.lines);

    for (const r of block.regions) {
      regions.push({
        lineNumber: startLine + r.offsetFromStart,
        endLineNumber: startLine + r.endOffsetFromStart,
        text: r.text,
        type: r.type,
      });
    }
  }

  return {
    markdown: allLines.join('\n'),
    regions,
  };
}
