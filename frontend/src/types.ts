// Types matching Go JSON API responses

export interface APIBadge {
  text: string;
  color: string;
  bg: string;
  activeBg?: string;
  activeColor?: string;
}

export interface APIProject {
  name: string;
  qualifiedName: string;
  workspace: string;
  workspacePath?: string;
  projectPath: string;
  origin: 'workspace' | 'standalone';
  badges: APIBadge[];
  branch?: string;
  dirty?: boolean;
  fileCount: number;
  lastModified: string;
  agentConnected?: boolean;
  agentRunning?: boolean;
  age?: string;
  reviewCount?: number;
}

export interface APIFile {
  name: string;
  title?: string;
  path: string;
  dir?: string;
  source?: string;
  sourceType?: string;
  sourceAuto?: boolean;
  project?: string;
  workspace?: string;
  age: string;
  fileType?: string;
  activityType?: string;
  activityAge?: string;
}

export interface APIFileGroupView {
  name: string;
  source: string;
  sourceType: string;
  auto: boolean;
  badgeText?: string;
  badgeColor?: string;
  badgeBg?: string;
  files: APIFile[];
}

export interface SvgRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Anchor {
  selectedText: string;
  before?: string;
  after?: string;
  headingPath?: string;
  startLine?: number;
  svgSnippet?: string;
  svgRect?: SvgRect;
}

export interface Comment {
  id: string;
  author: string;
  role: 'human' | 'agent';
  body: string;
  createdAt: string;
  suggestedReplies?: string[];
  inReplyTo?: string;
}

export interface Thread {
  id: string;
  status: 'open' | 'resolved';
  anchor: Anchor;
  comments: Comment[];
  createdAt: string;
  resolvedAt?: string;
  resolvedBy?: string;
}

export interface ThreadResponse extends Thread {
  agentTyping?: boolean;
}

export interface ThreadWithFile extends Thread {
  filePath: string;
}

export interface APIFileInReview {
  filePath: string;
  openThreads: number;
  agentActive: boolean;
  typingThreads?: number;
}

export interface ReviewFileEntry {
  name: string;
  title?: string;
  path: string;
  project: string;
  projectPath: string;
  openThreads: number;
  agentActive: boolean;
  typingThreads?: number;
  fileType?: string;
  age?: string;
  source?: string;
  sourceType?: string;
}

export interface ReviewGroup {
  workspace?: string;
  workspaceURL?: string;
  projectName: string;
  projectQN: string;
  sourceName: string;
  sourceAnchor: string;
  badgeText?: string;
  badgeColor?: string;
  badgeBg?: string;
  agentActive: boolean;
  typingThreads?: number;
  files: ReviewFileEntry[];
}

export interface ProjectInfo {
  fileCount: number;
  dirty: boolean;
  unpushedCommits: number;
}

export interface AgentStatus {
  project: string;
  pid: number;
  startedAt: string;
  running: boolean;
  contextWindow: number;
  contextUsed: number;
  contextPercent: number;
  totalCostUSD: number;
  numTurns: number;
  needsAgent?: boolean;
}

export interface PublishState {
  siteName: string;
  url: string;
  lastPublished: string;
}

export interface SSEEvent {
  type: 'projects' | 'files' | 'comments' | 'agents' | 'navigate';
  project?: string;
  path?: string;
}

// Search types
export interface SearchMatchedFile {
  path: string;
  name: string;
  title?: string;
  nameMatch?: boolean;
  fileType: string;
}

export interface SearchProjectResults {
  project: string;
  qualifiedName: string;
  workspace?: string;
  projectPath: string;
  files?: SearchMatchedFile[];
}

export interface SearchResponse {
  query: string;
  matchingProjects?: SearchProjectResults[];
  projectResults?: SearchProjectResults[];
  totalFiles: number;
}

// Request types
export interface CreateThreadReq {
  project: string;
  path: string;
  anchor: Anchor;
  author: string;
  role: string;
  body: string;
  suggestedReplies?: string[];
}

export interface ReplyReq {
  project: string;
  path: string;
  author: string;
  role: string;
  body: string;
  suggestedReplies?: string[];
}

export interface PatchThreadReq {
  project: string;
  path: string;
  status: 'resolved' | 'open';
  resolvedBy?: string;
}
