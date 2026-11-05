//! Catch-up for an attach: recording what a `session/load` replay, and the
//! live stream after it, add to a transcript that already holds part of the
//! session.
//!
//! The replay re-sends the whole session. Everything the transcript already
//! holds is dropped by identity: text by its ACP message id (or, for an agent
//! that sends none, by position in the stored text), tool calls by tool-call
//! id. A tool call's updates are dropped only once the transcript holds the
//! call's final state: the call that was running at the disconnect has its
//! updates replayed, and they are new. The replay can end mid-turn — its last
//! tool call may be `pending`, with the result still to come live — so nothing
//! waits for the replay to "complete"; every frame stands on its own. Once the
//! load answers, every frame is the turn's live stream, recorded as a live
//! turn records it.
//!
//! The replay also says whether the agent ever received the newest prompt the
//! transcript holds ([`LatestPrompt`]): Staged persists a prompt before it
//! sends it, so a connection lost in between leaves a prompt in the transcript
//! that the agent's session does not have.
use super::*;

use agent_client_protocol::schema::v1::ToolCallStatus;

/// Which part of an attach a frame arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatchUpStage {
    /// Before `session/load`: the in-flight turn's frames the proxy relayed
    /// as soon as it attached, which the own-session gate held until the
    /// session was named. The replay re-sends everything they describe, in
    /// transcript order, so they are dropped: recorded here they would land
    /// ahead of the older turns the replay is about to backfill. The one
    /// exceptions are text of a message still streaming, which an agent that
    /// writes messages to its transcript only once complete (the Claude
    /// bridge) cannot replay: that is kept, and recorded ahead of the
    /// message's next live chunk, or once the replay is over; and an update
    /// that settles a tool call, which an agent can send live only (the
    /// Claude bridge closes the calls a cancel left open without writing a
    /// result): that is kept for a call the replay leaves running (see
    /// [`CatchUpLedger::go_live`]).
    Held,
    /// `session/load` is out: the replay.
    Replay,
    /// The load answered: the turn's live stream.
    Live,
}

/// Whether an attach's replay showed the agent holding the newest prompt the
/// transcript has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatestPrompt {
    /// The replay re-sent it: the agent received it, so the turn the session
    /// ended on is this prompt's.
    Replayed,
    /// The replay ends on the prompt before this one: the agent never
    /// received it (the connection went between Staged persisting the prompt
    /// and sending it). Whatever turn the session shows ended is an earlier
    /// one.
    Missing,
    /// Nothing to tell from: the transcript holds no prompt text, the agent
    /// replays no prompts at all, or the replay lacks this one without
    /// proving it never arrived (it ends on something else, or the prompt is
    /// a slash command the agent may not replay as typed).
    Unknown,
}

/// The stored text of one message, matched against its replay by ACP message
/// id: the replay re-sends the message whole, from its start.
#[derive(Debug, Default)]
struct StoredText {
    text: String,
    matched: usize,
    /// The replay has gone past (or away from) the stored text: everything
    /// from here on is new.
    exhausted: bool,
}

impl StoredText {
    fn new(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }

    /// The part of replayed `chunk` the transcript does not hold yet.
    ///
    /// A chunk that continues the stored text where the replay left off is
    /// already stored; one that runs past its end is stored up to there (a
    /// message cut short by the disconnect); one that does not line up is
    /// new text, and so is the rest of this message after it. Only this
    /// message: the next one is matched by its own id.
    fn unseen<'a>(&mut self, chunk: &'a str) -> Option<&'a str> {
        if self.exhausted {
            return Some(chunk);
        }
        let rest = &self.text[self.matched..];
        if rest.starts_with(chunk) {
            self.matched += chunk.len();
            return None;
        }
        self.exhausted = true;
        let tail = chunk.strip_prefix(rest).unwrap_or(chunk);
        (!tail.is_empty()).then_some(tail)
    }
}

/// Assistant text stored without message ids, as one stream in transcript
/// order, matched against the replay by position.
///
/// With no ids, nothing marks where one message ends and the next begins, so
/// a replayed chunk that does not line up says nothing about the chunks after
/// it: the transcript may span more than one agent session (the replay
/// re-sends only the last), or the agent may replay text differently from how
/// it streamed it. Neither may make the rest of the session's history new.
#[derive(Debug, Default)]
struct PositionalText {
    text: String,
    /// Bytes of `text` the replay has accounted for.
    matched: usize,
}

impl PositionalText {
    fn new(text: String) -> Self {
        Self { text, matched: 0 }
    }

    /// The part of replayed `chunk` the transcript does not hold yet.
    ///
    /// A chunk that continues the stored text is stored, and one that runs
    /// past its end is new past it. A chunk that does not line up is looked
    /// for further on: found, the replay skipped stored text (an earlier
    /// agent session's), and the walk re-syncs past it. Found nowhere, the
    /// replay is telling stored text differently: the chunk is counted
    /// against the stored text by length, and only what runs past the
    /// stored length is new.
    fn unseen<'a>(&mut self, chunk: &'a str) -> Option<&'a str> {
        let rest = &self.text[self.matched..];
        if rest.is_empty() {
            return (!chunk.is_empty()).then_some(chunk);
        }
        if let Some(tail) = chunk.strip_prefix(rest) {
            self.matched = self.text.len();
            return (!tail.is_empty()).then_some(tail);
        }
        if rest.starts_with(chunk) {
            self.matched += chunk.len();
            return None;
        }
        if let Some(at) = rest.find(chunk) {
            self.matched += at + chunk.len();
            return None;
        }
        if chunk.len() <= rest.len() {
            let mut end = self.matched + chunk.len();
            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
            self.matched = end;
            return None;
        }
        let mut split = rest.len();
        while !chunk.is_char_boundary(split) {
            split += 1;
        }
        self.matched = self.text.len();
        let tail = &chunk[split..];
        (!tail.is_empty()).then_some(tail)
    }
}

/// Text held from before the load for one message.
#[derive(Debug)]
struct HeldMessage {
    /// The first held chunk: the held text is recorded with its metadata.
    first: ContentChunk,
    text: String,
}

/// The prompts the transcript holds, against the user messages the replay
/// re-sends, to tell whether the newest prompt reached the agent.
///
/// A replayed prompt holds its stored text (the agent also gets the notes
/// Staged appends for attachments), and the replay re-sends prompts in
/// order, but it need not re-send them all: an agent that compacted its
/// context replays from the compaction on, and agents add user messages of
/// their own (task notifications, interrupt markers). So the stored prompts
/// are aligned with the replay from the newest back, twice: once as if the
/// newest prompt arrived, once as if it never did. Whichever accounts for
/// more of the stored prompts says which happened; a tie (a newest prompt
/// that repeats the one before it, with the replay too short to tell them
/// apart) says nothing.
#[derive(Debug, Default)]
struct PromptLedger {
    /// The prompts Staged stored, oldest first, normalized; prompts with no
    /// text (an image alone) are left out, as the replay carries no text for
    /// them either.
    stored: Vec<String>,
    /// The newest stored prompt has no text: nothing to match it by.
    latest_textless: bool,
    /// The user messages the replay re-sent, normalized.
    replayed: Vec<String>,
    /// The replayed user message being assembled from its chunks: its message
    /// id, and its text.
    open: Option<(Option<String>, String)>,
}

impl PromptLedger {
    /// Only prompts Staged wrote itself count: rows without an ACP message
    /// id. A recorded `user_message_chunk` (a steer's echo) is the agent's
    /// copy of one of them, not another prompt.
    fn new(stored: &[ReplayBoundary]) -> Self {
        let prompts: Vec<String> = stored
            .iter()
            .filter(|boundary| {
                boundary.role == "user"
                    && boundary.acp_message_id.is_none()
                    && boundary.acp_tool_call_id.is_none()
            })
            .map(|boundary| normalize_prompt(&boundary.content))
            .collect();
        let latest_textless = prompts.last().is_some_and(String::is_empty);
        Self {
            stored: prompts
                .into_iter()
                .filter(|prompt| !prompt.is_empty())
                .collect(),
            latest_textless,
            ..Self::default()
        }
    }

    /// A replayed user message chunk. Chunks run together into one message
    /// until something else is replayed, or a chunk names another message.
    fn push(&mut self, message_id: Option<&str>, text: &str) {
        if let Some((open_id, _)) = &self.open {
            if open_id.is_some() && message_id.is_some() && open_id.as_deref() != message_id {
                self.close();
            }
        }
        let (_, open) = self
            .open
            .get_or_insert_with(|| (message_id.map(str::to_string), String::new()));
        open.push_str(text);
    }

    /// The user message being assembled is complete.
    fn close(&mut self) {
        if let Some((_, text)) = self.open.take() {
            self.replayed.push(normalize_prompt(&text));
        }
    }

    fn verdict(&self) -> LatestPrompt {
        if self.latest_textless || self.stored.is_empty() || self.replayed.is_empty() {
            return LatestPrompt::Unknown;
        }
        let received = aligned(&self.stored, &self.replayed);
        let never_sent = aligned(&self.stored[..self.stored.len() - 1], &self.replayed);
        match received.cmp(&never_sent) {
            std::cmp::Ordering::Greater => LatestPrompt::Replayed,
            std::cmp::Ordering::Less if self.replay_ends_before_newest() => LatestPrompt::Missing,
            std::cmp::Ordering::Less | std::cmp::Ordering::Equal => LatestPrompt::Unknown,
        }
    }

    /// The positive evidence `Missing` needs, on top of the alignment
    /// preferring it: the replay's last user message is the prompt before the
    /// newest, so nothing the user sent came after it, and the newest prompt
    /// is one the replay would show if the agent had received it.
    ///
    /// `Missing` turns a turn that may have finished into a resumable stop
    /// (no hooks, no queue drain), so a replay that merely lacks the newest
    /// prompt's text is not enough. The agent may have rewritten it, or
    /// added a user message of its own after it (an interrupt marker), and a
    /// slash command may never be replayed as typed at all: the Claude bridge
    /// leaves the local-only ones out of its replay (`/compact`, `/context`,
    /// `/model`, `/status`, ...) and the SDK expands prompt commands into
    /// their prompt.
    ///
    /// The cost of erring this way, accepted because a false `Missing` is the
    /// dearer mistake: when the previous turn's transcript ends with a user
    /// message the agent added itself (a task notification once a background
    /// task finished, or an interrupt marker), a newest prompt that never
    /// reached the agent also reads `Unknown`, and reconcile then settles
    /// from the previous turn's result, `completed` for a task-notification
    /// tail. That is the setup-kill case `Missing` exists for, in a narrower
    /// form.
    fn replay_ends_before_newest(&self) -> bool {
        let [.., before_newest, newest] = self.stored.as_slice() else {
            return false;
        };
        !newest.starts_with('/')
            && self
                .replayed
                .last()
                .is_some_and(|last| last.contains(before_newest.as_str()))
    }
}

/// The tool calls whose final state `stored` holds: a call row with a status
/// that ends the call. A call stored with no status at all (an agent that
/// sends none) counts as settled once its result is stored, as nothing in a
/// replay could move it on; so does a result stored with no call row.
///
/// A result alone says nothing for a call with a status: the Claude bridge
/// records a shell command's description as the call's first content (and
/// so its result row) while the command is still running.
fn settled_tool_calls(stored: &[ReplayBoundary]) -> HashSet<String> {
    let mut statuses: HashMap<&str, Option<&str>> = HashMap::new();
    let mut results: HashSet<&str> = HashSet::new();
    for boundary in stored {
        match (boundary.role.as_str(), boundary.acp_tool_call_id.as_deref()) {
            ("tool_call", Some(id)) => {
                statuses.insert(id, boundary.acp_tool_status.as_deref());
            }
            ("tool_result", Some(id)) => {
                results.insert(id);
            }
            _ => {}
        }
    }
    let settled_calls = statuses
        .iter()
        .filter(|(id, status)| match status {
            Some(status) => is_final_tool_status(status),
            None => results.contains(*id),
        })
        .map(|(id, _)| *id);
    let orphan_results = results
        .iter()
        .copied()
        .filter(|id| !statuses.contains_key(id));
    settled_calls
        .chain(orphan_results)
        .map(str::to_string)
        .collect()
}

/// Whether a stored ACP tool status ends the call.
fn is_final_tool_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "canceled")
}

/// Whether a replayed frame's status ends the call.
fn settles_tool_call(status: Option<&ToolCallStatus>) -> bool {
    matches!(
        status,
        Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
    )
}

/// The tool call `update` settles, if it settles one.
fn settled_call_id(update: &SessionUpdate) -> Option<String> {
    let (id, status) = match update {
        SessionUpdate::ToolCall(call) => (&call.tool_call_id, Some(&call.status)),
        SessionUpdate::ToolCallUpdate(update) => {
            (&update.tool_call_id, update.fields.status.as_ref())
        }
        _ => return None,
    };
    settles_tool_call(status).then(|| id.0.to_string())
}

/// How many of `stored`'s prompts, counted back from the newest, the replay
/// re-sent in order: each matched to the latest replayed message before the
/// one matched to the prompt after it that holds its text, until one isn't
/// found (the replay starts after it).
fn aligned(stored: &[String], replayed: &[String]) -> usize {
    let mut before = replayed.len();
    let mut matched = 0;
    for prompt in stored.iter().rev() {
        let Some(at) = replayed[..before]
            .iter()
            .rposition(|message| message.contains(prompt.as_str()))
        else {
            break;
        };
        before = at;
        matched += 1;
    }
    matched
}

/// A prompt compared by its words: the agent sends attachment notes after
/// the text Staged stored, and may wrap or re-flow it.
fn normalize_prompt(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `chunk` carrying `text` in place of its own.
fn with_text(chunk: &ContentChunk, text: &str) -> ContentChunk {
    let mut chunk = chunk.clone();
    chunk.content = AcpContentBlock::Text(TextContent::new(text));
    chunk
}

/// Record `text` as agent message text, with `chunk`'s metadata.
fn append(chunk: &ContentChunk, text: &str) -> LiveAction {
    LiveAction::AppendText {
        text: text.to_string(),
        metadata: message_chunk_metadata("agent_message_chunk", &with_text(chunk, text)),
    }
}

/// An attach's catch-up state (see the module docs).
pub(super) struct CatchUpLedger {
    stage: CatchUpStage,
    /// Stored text per ACP message id: user and assistant alike, since both
    /// replay as message chunks.
    messages: HashMap<String, StoredText>,
    /// Assistant text stored without a message id, matched by position.
    unidentified: PositionalText,
    /// Tool calls the transcript holds, or this attach recorded.
    tool_calls: HashSet<String>,
    /// Tool calls whose final state the transcript holds, or this attach
    /// recorded: nothing the replay re-sends for them is new (see
    /// [`settled_tool_calls`]).
    settled_tool_calls: HashSet<String>,
    /// The replay has produced something the transcript did not hold, so the
    /// events it carries from here on (usage, plans) are new too.
    past_boundary: bool,
    /// Held text per message id, and the order the messages first appeared.
    /// An entry goes once the replay stage shows what its message is.
    held: HashMap<String, HeldMessage>,
    held_order: Vec<String>,
    /// Held updates that settle a tool call: the first per call, in arrival
    /// order.
    held_settlements: Vec<(String, SessionUpdate)>,
    /// Messages with held text that went on streaming live beside the
    /// replay, which does not re-send them: their text is new as it comes.
    streaming: HashSet<String>,
    prompts: PromptLedger,
}

impl CatchUpLedger {
    pub(super) fn new(stored: &[ReplayBoundary]) -> Self {
        let mut messages: HashMap<String, String> = HashMap::new();
        let mut unidentified = String::new();
        let mut tool_calls = HashSet::new();
        for boundary in stored {
            match (boundary.role.as_str(), &boundary.acp_tool_call_id) {
                ("tool_call", Some(id)) => {
                    tool_calls.insert(id.clone());
                }
                ("tool_result", Some(_)) => {}
                _ => match &boundary.acp_message_id {
                    Some(id) => messages
                        .entry(id.clone())
                        .or_default()
                        .push_str(&boundary.content),
                    None if boundary.role == "assistant" => {
                        unidentified.push_str(&boundary.content)
                    }
                    None => {}
                },
            }
        }
        Self {
            stage: CatchUpStage::Held,
            messages: messages
                .into_iter()
                .map(|(id, text)| (id, StoredText::new(text)))
                .collect(),
            unidentified: PositionalText::new(unidentified),
            tool_calls,
            settled_tool_calls: settled_tool_calls(stored),
            past_boundary: false,
            held: HashMap::new(),
            held_order: Vec::new(),
            held_settlements: Vec::new(),
            streaming: HashSet::new(),
            prompts: PromptLedger::new(stored),
        }
    }

    pub(super) fn set_stage(&mut self, stage: CatchUpStage) {
        self.stage = stage;
    }

    /// The replay is over; from here every frame is the turn's live stream.
    /// Returns what to record before the next live frame: the held text of
    /// each message the replay stage carried nothing for, in the order the
    /// messages first appeared, then the held settling of each call the
    /// transcript or the replay holds that is still running.
    pub(super) fn go_live(&mut self) -> Vec<SessionUpdate> {
        self.prompts.close();
        self.stage = CatchUpStage::Live;
        let mut held = std::mem::take(&mut self.held);
        let text = std::mem::take(&mut self.held_order)
            .into_iter()
            .filter_map(|id| held.remove(&id))
            .filter(|message| !message.text.is_empty())
            .map(|message| {
                SessionUpdate::AgentMessageChunk(with_text(&message.first, &message.text))
            });
        let settlements = std::mem::take(&mut self.held_settlements)
            .into_iter()
            .filter(|(id, _)| self.tool_calls.contains(id) && !self.settled_tool_calls.contains(id))
            .map(|(_, update)| update);
        text.chain(settlements).collect()
    }

    /// Whether the replay re-sent the newest stored prompt (see
    /// [`LatestPrompt`]). Meaningful once the replay is over.
    pub(super) fn latest_prompt(&self) -> LatestPrompt {
        self.prompts.verdict()
    }

    /// Whether a `result` frame may close a response: not one held from
    /// before the load, which would be recorded ahead of the replay.
    pub(super) fn accepts_results(&self) -> bool {
        self.stage != CatchUpStage::Held
    }

    /// What `update` records.
    pub(super) fn action_for(&mut self, update: &SessionUpdate) -> LiveAction {
        match self.stage {
            CatchUpStage::Held => {
                self.hold(update);
                return LiveAction::Drop;
            }
            // Nothing live is a copy of something stored: a tool call's
            // updates in particular must all land, as in a live turn. The
            // Claude bridge's first update of a call carries its input (a
            // shell command's description) as content, which records as the
            // call's result until the real one replaces it.
            CatchUpStage::Live => return live_action_for_update(update),
            CatchUpStage::Replay => {}
        }
        match update {
            SessionUpdate::UserMessageChunk(chunk) => {
                if let AcpContentBlock::Text(text) = &chunk.content {
                    self.prompts.push(
                        chunk.message_id.as_ref().map(|id| id.0.as_ref()),
                        &text.text,
                    );
                }
            }
            _ => self.prompts.close(),
        }
        if let Some(action) = self.held_message_text(update) {
            return action;
        }
        match update {
            SessionUpdate::ToolCall(tool_call) => {
                let id = tool_call.tool_call_id.0.to_string();
                let settles = settles_tool_call(Some(&tool_call.status));
                let new_call = self.tool_calls.insert(id.clone());
                // A call the transcript holds is re-sent whole: only its
                // settling, by an agent that settles a call by re-sending
                // it, is new.
                if !new_call && !(settles && !self.settled_tool_calls.contains(&id)) {
                    return LiveAction::Drop;
                }
                if settles {
                    self.settled_tool_calls.insert(id);
                }
                self.past_boundary = true;
                live_action_for_update(update)
            }
            SessionUpdate::ToolCallUpdate(tool_update) => {
                let id = tool_update.tool_call_id.0.to_string();
                if self.settled_tool_calls.contains(&id) {
                    return LiveAction::Drop;
                }
                // The call is new, or was still running when the transcript
                // was written: its updates up to its settling are new (the
                // writer updates the rows the transcript holds for it).
                let action = live_action_for_update(update);
                let settles = settles_tool_call(tool_update.fields.status.as_ref());
                if settles {
                    self.settled_tool_calls.insert(id);
                }
                if settles
                    || matches!(
                        &action,
                        LiveAction::ToolCallUpdate {
                            result: Some(_),
                            ..
                        }
                    )
                {
                    self.past_boundary = true;
                }
                action
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                let AcpContentBlock::Text(text) = &chunk.content else {
                    return self.past_boundary_event(update);
                };
                let unseen = match &chunk.message_id {
                    Some(id) => self
                        .messages
                        .entry(id.0.to_string())
                        .or_default()
                        .unseen(&text.text),
                    None => self.unidentified.unseen(&text.text),
                };
                let Some(unseen) = unseen else {
                    return LiveAction::Drop;
                };
                self.past_boundary = true;
                append(chunk, unseen)
            }
            // The replayed prompts are the transcript's own user rows.
            SessionUpdate::UserMessageChunk(_) => LiveAction::Drop,
            _ => self.past_boundary_event(update),
        }
    }

    /// Keep a held text chunk that names its message, in case the replay
    /// never re-sends that message, and a held update that settles a tool
    /// call, in case the replay leaves the call running.
    fn hold(&mut self, update: &SessionUpdate) {
        if let Some(id) = settled_call_id(update) {
            if !self.held_settlements.iter().any(|(held, _)| *held == id) {
                self.held_settlements.push((id, update.clone()));
            }
            return;
        }
        let SessionUpdate::AgentMessageChunk(chunk) = update else {
            return;
        };
        let (AcpContentBlock::Text(text), Some(id)) = (&chunk.content, &chunk.message_id) else {
            return;
        };
        let id = id.0.to_string();
        match self.held.get_mut(&id) {
            Some(message) => message.text.push_str(&text.text),
            None => {
                self.held_order.push(id.clone());
                self.held.insert(
                    id,
                    HeldMessage {
                        first: chunk.clone(),
                        text: text.text.clone(),
                    },
                );
            }
        }
    }

    /// What a replay-stage text chunk records for a message with text held
    /// from before the load; `None` to handle it as any replayed chunk.
    ///
    /// The first such chunk tells which the message is. The replay re-sends
    /// a message from its start, so a chunk that lines up with the start of
    /// the stored text (or, with none stored, holds all the held text) is
    /// the replay's copy, held part included: the held copy is dropped, and
    /// the chunk walked like any other. Anything else is the message
    /// streaming on live beside the replay, which will not re-send it: the
    /// held text is recorded ahead of the chunk, and the rest of the message
    /// is new as it comes.
    fn held_message_text(&mut self, update: &SessionUpdate) -> Option<LiveAction> {
        let SessionUpdate::AgentMessageChunk(chunk) = update else {
            return None;
        };
        let (AcpContentBlock::Text(text), Some(id)) = (&chunk.content, &chunk.message_id) else {
            return None;
        };
        let id = id.0.as_ref();
        if self.streaming.contains(id) {
            return Some(append(chunk, &text.text));
        }
        let held = self.held.remove(id)?;
        let resent = match self
            .messages
            .get(id)
            .map(|stored| stored.text.as_str())
            .filter(|stored| !stored.is_empty())
        {
            Some(stored) => stored.starts_with(&text.text) || text.text.starts_with(stored),
            None => text.text.contains(&held.text),
        };
        if resent {
            return None;
        }
        self.streaming.insert(id.to_string());
        Some(append(chunk, &format!("{}{}", held.text, text.text)))
    }

    /// A replayed event with no identity of its own: new only once the replay
    /// has passed the stored transcript.
    fn past_boundary_event(&self, update: &SessionUpdate) -> LiveAction {
        if self.past_boundary {
            live_action_for_update(update)
        } else {
            LiveAction::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        ToolCall, ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
    };

    fn boundary(
        role: &str,
        content: &str,
        message: Option<&str>,
        tool: Option<&str>,
    ) -> ReplayBoundary {
        ReplayBoundary {
            role: role.to_string(),
            content: content.to_string(),
            acp_message_id: message.map(str::to_string),
            acp_tool_call_id: tool.map(str::to_string),
            acp_tool_status: None,
        }
    }

    /// A stored tool call row with its last ACP status.
    fn stored_call(id: &str, status: Option<&str>) -> ReplayBoundary {
        ReplayBoundary {
            acp_tool_status: status.map(str::to_string),
            ..boundary("tool_call", "Run", None, Some(id))
        }
    }

    fn prompt(content: &str) -> ReplayBoundary {
        boundary("user", content, None, None)
    }

    fn text(message_id: Option<&str>, text: &str) -> SessionUpdate {
        let mut chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(text)));
        chunk.message_id = message_id.map(|id| id.to_string().into());
        SessionUpdate::AgentMessageChunk(chunk)
    }

    fn user_text(message_id: Option<&str>, text: &str) -> SessionUpdate {
        let mut chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(text)));
        chunk.message_id = message_id.map(|id| id.to_string().into());
        SessionUpdate::UserMessageChunk(chunk)
    }

    fn tool_call(id: &str) -> SessionUpdate {
        SessionUpdate::ToolCall(
            ToolCall::new(ToolCallId::new(id), "Run").status(ToolCallStatus::Pending),
        )
    }

    fn tool_result(id: &str) -> SessionUpdate {
        tool_update(id, Some(ToolCallStatus::Completed), Some("done"))
    }

    fn tool_update(
        id: &str,
        status: Option<ToolCallStatus>,
        content: Option<&str>,
    ) -> SessionUpdate {
        let mut fields = ToolCallUpdateFields::new();
        if let Some(status) = status {
            fields = fields.status(status);
        }
        if let Some(content) = content {
            fields = fields.content(vec![content.to_string().into()]);
        }
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(ToolCallId::new(id), fields))
    }

    /// The result and status a tool call action records, or `None` for one
    /// that records nothing.
    fn recorded_tool(action: LiveAction) -> Option<(Option<String>, Option<String>)> {
        match action {
            LiveAction::ToolCallUpdate {
                result, metadata, ..
            } => Some((result, metadata.tool_status)),
            LiveAction::RecordToolCall { metadata, .. } => Some((None, metadata.tool_status)),
            LiveAction::Drop => None,
            _ => panic!("expected a tool call action or nothing"),
        }
    }

    fn appended(action: LiveAction) -> Option<String> {
        match action {
            LiveAction::AppendText { text, metadata } => {
                let stored: serde_json::Value = metadata.content.expect("chunk content");
                assert_eq!(stored["content"]["text"], text, "metadata carries the tail");
                Some(text)
            }
            LiveAction::Drop => None,
            _ => panic!("expected text or nothing"),
        }
    }

    /// The text of the updates `go_live` hands back, in order.
    fn texts(updates: &[SessionUpdate]) -> Vec<&str> {
        updates
            .iter()
            .map(|update| match update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: AcpContentBlock::Text(text),
                    ..
                }) => text.text.as_str(),
                _ => panic!("expected a text chunk"),
            })
            .collect()
    }

    fn replaying(stored: &[ReplayBoundary]) -> CatchUpLedger {
        let mut ledger = CatchUpLedger::new(stored);
        ledger.set_stage(CatchUpStage::Replay);
        ledger
    }

    #[test]
    fn stored_text_is_dropped_and_a_cut_message_keeps_only_its_tail() {
        let mut ledger = replaying(&[
            boundary("assistant", "Hello ", Some("m1"), None),
            boundary("assistant", "done", Some("m0"), None),
        ]);
        assert_eq!(appended(ledger.action_for(&text(Some("m0"), "done"))), None);
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "Hello world"))),
            Some("world".to_string())
        );
        assert_eq!(
            appended(ledger.action_for(&text(Some("m2"), "new"))),
            Some("new".to_string())
        );
    }

    #[test]
    fn a_message_replayed_in_blocks_is_walked_block_by_block() {
        let mut ledger = replaying(&[boundary("assistant", "one two", Some("m1"), None)]);
        assert_eq!(appended(ledger.action_for(&text(Some("m1"), "one "))), None);
        assert_eq!(appended(ledger.action_for(&text(Some("m1"), "two"))), None);
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), " three"))),
            Some(" three".to_string())
        );
    }

    #[test]
    fn text_without_ids_is_matched_by_position() {
        let mut ledger = replaying(&[
            boundary("assistant", "first", None, None),
            boundary("user", "ignored", None, None),
            boundary("assistant", "second", None, None),
        ]);
        assert_eq!(appended(ledger.action_for(&text(None, "first"))), None);
        assert_eq!(appended(ledger.action_for(&text(None, "sec"))), None);
        assert_eq!(
            appended(ledger.action_for(&text(None, "ond, then more"))),
            Some(", then more".to_string()),
            "text past the stored stream is new"
        );
        assert_eq!(
            appended(ledger.action_for(&text(None, "first"))),
            Some("first".to_string()),
            "and so is everything after it"
        );
    }

    /// A transcript that spans two agent sessions holds text the replay
    /// (of the last session only) never re-sends: the walk skips it.
    #[test]
    fn text_without_ids_resyncs_past_stored_text_the_replay_skips() {
        let mut ledger = replaying(&[
            boundary("assistant", "From the earlier agent session. ", None, None),
            boundary("assistant", "From this one. ", None, None),
            boundary("assistant", "And its next reply.", None, None),
        ]);
        assert_eq!(
            appended(ledger.action_for(&text(None, "From this one. "))),
            None
        );
        assert_eq!(
            appended(ledger.action_for(&text(None, "And its next reply."))),
            None
        );
        assert_eq!(
            appended(ledger.action_for(&text(None, " Then the catch-up."))),
            Some(" Then the catch-up.".to_string())
        );
    }

    /// Text the agent replays differently from how it streamed it is
    /// counted against the stored text, never taken as the start of new text
    /// that runs to the end of the replay.
    #[test]
    fn misaligned_text_without_ids_is_counted_by_length() {
        let stored = "Line one.\n\nLine two.";
        let mut ledger = replaying(&[boundary("assistant", stored, None, None)]);
        // Replayed re-flowed: it lines up nowhere in the stored text.
        assert_eq!(
            appended(ledger.action_for(&text(None, "Line one. Line two."))),
            None
        );
        // One stored byte is left: a chunk that runs past it is new only past
        // it, and everything after is new.
        assert_eq!(
            appended(ledger.action_for(&text(None, "!New."))),
            Some("New.".to_string())
        );
        assert_eq!(
            appended(ledger.action_for(&text(None, " More."))),
            Some(" More.".to_string())
        );
    }

    #[test]
    fn counting_by_length_splits_on_a_char_boundary() {
        let mut stream = PositionalText::new("héllo".to_string());
        // "zzzzz" fills 5 of the 6 stored bytes; the next chunk's first byte
        // is the last stored one, its second starts a two-byte char.
        assert_eq!(stream.unseen("zzzzz"), None);
        assert_eq!(stream.unseen("aé!"), Some("é!"));
        let mut stream = PositionalText::new("abc".to_string());
        assert_eq!(stream.unseen("é"), None, "two bytes fit in three");
        assert_eq!(stream.matched, 2);
        assert_eq!(stream.unseen("zé"), Some("é"));
    }

    #[test]
    fn tool_calls_dedupe_by_id_and_a_pending_call_completes_live() {
        let mut ledger = replaying(&[
            boundary("tool_call", "Run", None, Some("t1")),
            boundary("tool_result", "done", None, Some("t1")),
            boundary("tool_call", "Run", None, Some("t2")),
        ]);
        assert!(matches!(
            ledger.action_for(&tool_call("t1")),
            LiveAction::Drop
        ));
        assert!(matches!(
            ledger.action_for(&tool_result("t1")),
            LiveAction::Drop
        ));
        assert!(matches!(
            ledger.action_for(&tool_call("t2")),
            LiveAction::Drop
        ));
        // The call that was running at the disconnect: its result is new.
        assert!(matches!(
            ledger.action_for(&tool_result("t2")),
            LiveAction::ToolCallUpdate {
                result: Some(_),
                ..
            }
        ));
        // A call the replay ends on, still pending, and its live result.
        assert!(matches!(
            ledger.action_for(&tool_call("t3")),
            LiveAction::RecordToolCall { .. }
        ));
        ledger.set_stage(CatchUpStage::Live);
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_result("t3"))),
            Some((Some("done".to_string()), Some("completed".to_string())))
        );
    }

    /// The call that was running at the disconnect, as the Claude bridge
    /// leaves it stored: `pending`, with its first content (a shell
    /// command's description) as its result. Its replayed completion is
    /// new, output and status alike; once recorded, a copy is not.
    #[test]
    fn a_stored_call_still_running_takes_its_replayed_completion() {
        let mut ledger = replaying(&[
            stored_call("t1", Some("pending")),
            boundary("tool_result", "Run step 1", None, Some("t1")),
        ]);
        assert!(matches!(
            ledger.action_for(&tool_call("t1")),
            LiveAction::Drop
        ));
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_update(
                "t1",
                Some(ToolCallStatus::Completed),
                Some("step 1 done"),
            ))),
            Some((
                Some("step 1 done".to_string()),
                Some("completed".to_string())
            ))
        );
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_update(
                "t1",
                Some(ToolCallStatus::Completed),
                Some("step 1 done"),
            ))),
            None
        );

        // An agent that streams a call's output before settling it: every
        // update up to the settling lands, a status-only one included.
        let mut ledger = replaying(&[
            stored_call("t2", Some("in_progress")),
            boundary("tool_result", "line 1", None, Some("t2")),
        ]);
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_update(
                "t2",
                Some(ToolCallStatus::InProgress),
                Some("line 1\nline 2"),
            ))),
            Some((
                Some("line 1\nline 2".to_string()),
                Some("in_progress".to_string())
            ))
        );
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_update(
                "t2",
                Some(ToolCallStatus::Failed),
                None
            ))),
            Some((None, Some("failed".to_string())))
        );
    }

    /// A call is settled by its stored status, or (stored with none) by its
    /// stored result; its replayed frames are then all copies.
    #[test]
    fn a_settled_call_drops_everything_replayed_for_it() {
        let mut ledger = replaying(&[
            stored_call("done", Some("completed")),
            stored_call("broke", Some("failed")),
            stored_call("statusless", None),
            boundary("tool_result", "output", None, Some("statusless")),
            boundary("tool_result", "output", None, Some("orphan")),
        ]);
        for id in ["done", "broke", "statusless", "orphan"] {
            assert!(
                matches!(ledger.action_for(&tool_result(id)), LiveAction::Drop),
                "{id}"
            );
            assert!(
                matches!(
                    ledger.action_for(&tool_update(id, None, Some("more"))),
                    LiveAction::Drop
                ),
                "{id}"
            );
        }
        // Stored with no status and no result: still running for all the
        // transcript says.
        let mut ledger = replaying(&[stored_call("t1", None)]);
        assert!(recorded_tool(ledger.action_for(&tool_result("t1"))).is_some());
    }

    /// An agent that settles a call by re-sending it whole: the copy that
    /// settles a call the transcript holds unsettled is new.
    #[test]
    fn a_resent_call_that_settles_a_stored_running_one_is_recorded() {
        let mut ledger = replaying(&[stored_call("t1", Some("pending"))]);
        let settled = SessionUpdate::ToolCall(
            ToolCall::new(ToolCallId::new("t1"), "Run").status(ToolCallStatus::Completed),
        );
        assert_eq!(
            recorded_tool(ledger.action_for(&settled)),
            Some((None, Some("completed".to_string())))
        );
        assert_eq!(recorded_tool(ledger.action_for(&settled)), None);
        assert_eq!(recorded_tool(ledger.action_for(&tool_result("t1"))), None);
    }

    /// Settling the call that was running at the disconnect is where the
    /// replay passes the stored transcript: events after it are new.
    #[test]
    fn settling_the_running_call_passes_the_boundary() {
        let mut ledger = replaying(&[stored_call("t1", Some("pending"))]);
        let plan = SessionUpdate::Plan(agent_client_protocol::schema::v1::Plan::new(vec![]));
        assert!(matches!(ledger.action_for(&plan), LiveAction::Drop));
        // An update that neither settles the call nor carries a result says
        // nothing about where the replay is.
        ledger.action_for(&tool_update("t1", Some(ToolCallStatus::InProgress), None));
        assert!(matches!(ledger.action_for(&plan), LiveAction::Drop));
        ledger.action_for(&tool_result("t1"));
        assert!(matches!(
            ledger.action_for(&plan),
            LiveAction::RecordAcpEvent(_)
        ));
    }

    /// After the load every frame is live, recorded as a live turn records
    /// it: the Claude bridge's call, its refinement (whose content, the
    /// shell command's description, records as the result for now), and its
    /// completion, which must not be taken for a copy of the refinement.
    #[test]
    fn live_tool_calls_are_recorded_update_by_update() {
        let mut ledger = replaying(&[stored_call("t0", Some("completed"))]);
        ledger.go_live();
        assert!(recorded_tool(ledger.action_for(&tool_call("t1"))).is_some());
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_update("t1", None, Some("Run step 1")))),
            Some((Some("Run step 1".to_string()), None))
        );
        assert_eq!(
            recorded_tool(ledger.action_for(&tool_update(
                "t1",
                Some(ToolCallStatus::Completed),
                Some("step 1 done"),
            ))),
            Some((
                Some("step 1 done".to_string()),
                Some("completed".to_string())
            ))
        );
        // Live frames are never copies, even for a call the transcript holds
        // settled.
        assert!(recorded_tool(ledger.action_for(&tool_update("t0", None, Some("late")))).is_some());
    }

    #[test]
    fn held_frames_are_dropped_and_live_text_is_recorded_as_is() {
        let mut ledger = CatchUpLedger::new(&[boundary("assistant", "Hello ", Some("m1"), None)]);
        assert!(matches!(
            ledger.action_for(&tool_call("t9")),
            LiveAction::Drop
        ));
        assert!(matches!(
            ledger.action_for(&text(None, "no id")),
            LiveAction::Drop
        ));
        assert!(!ledger.accepts_results());
        ledger.set_stage(CatchUpStage::Replay);
        assert!(ledger.go_live().is_empty(), "nothing named was held");
        assert!(ledger.accepts_results());
        // A live delta of the stored message is new text, even where it
        // happens to repeat the stored text.
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "Hello"))),
            Some("Hello".to_string())
        );
        // The held tool call was not recorded, so the replay or the live
        // stream still can.
        assert!(matches!(
            ledger.action_for(&tool_call("t9")),
            LiveAction::RecordToolCall { .. }
        ));
    }

    /// The final message, still streaming when the attach connects: the
    /// agent has not written it to its transcript, so the replay has none of
    /// it, and the chunks relayed before the load are all there is of its
    /// middle. They go ahead of the message's next live chunk, even one that
    /// streams beside the replay.
    #[test]
    fn held_text_of_a_message_streaming_beside_the_replay_goes_ahead_of_it() {
        let mut ledger = CatchUpLedger::new(&[
            boundary("assistant", "Older reply.", Some("m0"), None),
            boundary("assistant", "The note: ", Some("m1"), None),
        ]);
        assert!(matches!(
            ledger.action_for(&text(Some("m1"), "first part, ")),
            LiveAction::Drop
        ));
        assert!(matches!(
            ledger.action_for(&text(Some("m1"), "second part, ")),
            LiveAction::Drop
        ));
        ledger.set_stage(CatchUpStage::Replay);
        assert_eq!(
            appended(ledger.action_for(&text(Some("m0"), "Older reply."))),
            None
        );
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "third part, "))),
            Some("first part, second part, third part, ".to_string())
        );
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "fourth part, "))),
            Some("fourth part, ".to_string()),
            "the rest of the message is new as it comes"
        );
        assert!(ledger.go_live().is_empty());
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "the end."))),
            Some("the end.".to_string())
        );
    }

    /// With nothing more of the message before the replay ends, its held text
    /// goes ahead of the live tail.
    #[test]
    fn held_text_of_a_message_the_replay_never_mentions_goes_ahead_of_the_live_tail() {
        let mut ledger = CatchUpLedger::new(&[
            boundary("assistant", "Older reply.", Some("m0"), None),
            boundary("assistant", "The note: ", Some("m1"), None),
        ]);
        ledger.action_for(&text(Some("m1"), "first part, "));
        ledger.action_for(&tool_call("t1"));
        ledger.action_for(&text(Some("m2"), "Another, "));
        ledger.action_for(&text(Some("m1"), "second part, "));
        ledger.set_stage(CatchUpStage::Replay);
        assert_eq!(
            appended(ledger.action_for(&text(Some("m0"), "Older reply."))),
            None
        );
        let pending = ledger.go_live();
        assert_eq!(
            texts(&pending),
            vec!["first part, second part, ", "Another, "]
        );
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "the end."))),
            Some("the end.".to_string())
        );
    }

    /// A message that finished before the load is replayed whole, held part
    /// included: the replay's copy is walked against the stored text, in
    /// place, and the held copy dropped.
    #[test]
    fn held_text_of_a_message_the_replay_resends_is_dropped() {
        let mut ledger = CatchUpLedger::new(&[boundary("assistant", "Hello ", Some("m1"), None)]);
        ledger.action_for(&text(Some("m1"), "wor"));
        ledger.set_stage(CatchUpStage::Replay);
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "Hello world"))),
            Some("world".to_string())
        );
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "!"))),
            Some("!".to_string())
        );
        assert!(ledger.go_live().is_empty());

        // With nothing stored for the message, the replay's copy is the one
        // that holds the held text.
        let mut ledger = CatchUpLedger::new(&[]);
        ledger.action_for(&text(Some("m1"), "wor"));
        ledger.set_stage(CatchUpStage::Replay);
        assert_eq!(
            appended(ledger.action_for(&text(Some("m1"), "Hello world"))),
            Some("Hello world".to_string())
        );
        assert!(ledger.go_live().is_empty());
    }

    /// A cancel sent before the load: the agent settles the running call
    /// live. The replay, from the agent's transcript, settles it too, and
    /// the held copy goes.
    #[test]
    fn a_held_settling_the_replay_repeats_is_dropped() {
        let mut ledger = CatchUpLedger::new(&[stored_call("t1", Some("pending"))]);
        let interrupted = tool_update("t1", Some(ToolCallStatus::Failed), Some("Interrupted"));
        assert!(matches!(ledger.action_for(&interrupted), LiveAction::Drop));
        ledger.set_stage(CatchUpStage::Replay);
        assert_eq!(
            recorded_tool(ledger.action_for(&interrupted)),
            Some((Some("Interrupted".to_string()), Some("failed".to_string())))
        );
        assert!(ledger.go_live().is_empty());
    }

    /// The agent settled a running call live only (the Claude bridge closes
    /// the calls a cancel left open), so the replay leaves it running: the
    /// held settling is recorded once the replay is over, after the held
    /// text. Settlings of calls nothing holds, or the transcript holds
    /// settled, are not.
    #[test]
    fn a_held_settling_of_a_call_the_replay_leaves_running_is_recorded_after_it() {
        let mut ledger = CatchUpLedger::new(&[
            stored_call("t0", Some("completed")),
            stored_call("t1", Some("in_progress")),
            boundary("assistant", "Hello ", Some("m1"), None),
        ]);
        let closed = tool_update("t1", Some(ToolCallStatus::Failed), Some("Cancelled"));
        for update in [
            text(Some("m1"), "wor"),
            tool_update("t1", None, Some("progress")),
            closed.clone(),
            tool_update("t1", Some(ToolCallStatus::Completed), Some("late")),
            tool_update("t0", Some(ToolCallStatus::Failed), None),
            tool_update("t9", Some(ToolCallStatus::Failed), None),
        ] {
            assert!(matches!(ledger.action_for(&update), LiveAction::Drop));
        }
        ledger.set_stage(CatchUpStage::Replay);
        assert!(recorded_tool(ledger.action_for(&tool_call("t1"))).is_none());
        let pending = ledger.go_live();
        assert_eq!(pending.len(), 2, "{pending:?}");
        assert_eq!(texts(&pending[..1]), vec!["wor"]);
        assert_eq!(pending[1], closed, "the first settling wins");
    }

    /// A call the replay first shows is held settled the same way.
    #[test]
    fn a_held_settling_of_a_replayed_call_left_running_is_recorded() {
        let mut ledger = CatchUpLedger::new(&[]);
        let closed = tool_update("t1", Some(ToolCallStatus::Failed), Some("Cancelled"));
        ledger.action_for(&closed);
        ledger.set_stage(CatchUpStage::Replay);
        assert!(recorded_tool(ledger.action_for(&tool_call("t1"))).is_some());
        assert_eq!(ledger.go_live(), vec![closed]);
    }

    #[test]
    fn replayed_prompts_and_events_before_the_boundary_are_dropped() {
        let mut ledger = replaying(&[boundary("assistant", "a", Some("m1"), None)]);
        assert!(matches!(
            ledger.action_for(&user_text(Some("u1"), "hi")),
            LiveAction::Drop
        ));
        let plan = SessionUpdate::Plan(agent_client_protocol::schema::v1::Plan::new(vec![]));
        assert!(matches!(ledger.action_for(&plan), LiveAction::Drop));
        assert!(appended(ledger.action_for(&text(Some("m2"), "b"))).is_some());
        assert!(matches!(
            ledger.action_for(&plan),
            LiveAction::RecordAcpEvent(_)
        ));
    }

    /// Replay `prompts` (one user message each, each answered), then end the
    /// replay and say what the ledger made of the newest stored prompt.
    fn latest_prompt_after(stored: &[ReplayBoundary], prompts: &[&str]) -> LatestPrompt {
        let mut ledger = replaying(stored);
        for (i, prompt) in prompts.iter().enumerate() {
            ledger.action_for(&user_text(Some(&format!("u{i}")), prompt));
            ledger.action_for(&text(Some(&format!("a{i}")), "reply"));
        }
        ledger.go_live();
        ledger.latest_prompt()
    }

    #[test]
    fn the_newest_prompt_in_the_replay_was_received() {
        let stored = [prompt("Fix the build"), prompt("Now add a test  for it")];
        // Re-flowed, and with the attachment notes Staged appends on send.
        assert_eq!(
            latest_prompt_after(
                &stored,
                &[
                    "Fix the build",
                    "Now add a test\nfor it\n\n[Attached: log.txt]"
                ]
            ),
            LatestPrompt::Replayed
        );
    }

    /// Staged persisted a resumed turn's prompt, then went away before the
    /// agent got it: the replay holds only the earlier turns.
    #[test]
    fn a_newest_prompt_the_replay_lacks_was_never_received() {
        let stored = [prompt("Fix the build"), prompt("Now add a test")];
        assert_eq!(
            latest_prompt_after(&stored, &["Fix the build"]),
            LatestPrompt::Missing
        );
    }

    /// A short prompt an earlier one happens to contain must be matched in
    /// order, not by any replayed prompt containing it.
    #[test]
    fn a_repeated_or_contained_newest_prompt_is_matched_in_order() {
        let stored = [
            prompt("continue with the plan"),
            prompt("yes"),
            prompt("continue"),
        ];
        assert_eq!(
            latest_prompt_after(&stored, &["continue with the plan", "yes"]),
            LatestPrompt::Missing
        );
        assert_eq!(
            latest_prompt_after(&stored, &["continue with the plan", "yes", "continue"]),
            LatestPrompt::Replayed
        );
        let stored = [prompt("fix it"), prompt("yes"), prompt("yes")];
        assert_eq!(
            latest_prompt_after(&stored, &["fix it", "yes"]),
            LatestPrompt::Missing
        );
        assert_eq!(
            latest_prompt_after(&stored, &["fix it", "yes", "yes"]),
            LatestPrompt::Replayed
        );
    }

    /// An agent that compacted its context replays from the compaction on:
    /// the older prompts are missing from the replay, which says nothing
    /// about the newest one.
    #[test]
    fn a_replay_that_starts_at_a_compaction_still_tells() {
        let stored = [
            prompt("Fix the build"),
            prompt("yes"),
            prompt("Now add a test"),
            prompt("yes"),
        ];
        let summary = "This session is being continued from a previous conversation.";
        assert_eq!(
            latest_prompt_after(&stored, &[summary, "Now add a test", "yes"]),
            LatestPrompt::Replayed
        );
        assert_eq!(
            latest_prompt_after(&stored, &[summary, "yes"]),
            LatestPrompt::Replayed,
            "the prompt before the newest was compacted away"
        );
        assert_eq!(
            latest_prompt_after(&stored, &[summary, "Now add a test"]),
            LatestPrompt::Missing
        );
        // Compacted down to one "yes": it could be either.
        let stored = [prompt("Fix the build"), prompt("yes"), prompt("yes")];
        assert_eq!(
            latest_prompt_after(&stored, &[summary, "yes"]),
            LatestPrompt::Unknown
        );
        // Compacted mid-turn: the replay holds no prompt at all.
        assert_eq!(
            latest_prompt_after(&stored, &[summary]),
            LatestPrompt::Unknown
        );
    }

    /// User messages the agent adds itself (task notifications, interrupt
    /// markers) are skipped when matching, but are no proof of a missing
    /// prompt.
    #[test]
    fn user_messages_the_agent_adds_are_skipped() {
        let stored = [prompt("Fix the build"), prompt("Now add a test")];
        assert_eq!(
            latest_prompt_after(
                &stored,
                &[
                    "Fix the build",
                    "<task-notification>build finished</task-notification>",
                    "Now add a test",
                    "[Request interrupted by user]",
                ]
            ),
            LatestPrompt::Replayed
        );
        // The replay doesn't end on the prompt before the newest: something
        // came after it, and the newest prompt may be what set it off.
        assert_eq!(
            latest_prompt_after(&stored, &["Fix the build", "[Request interrupted by user]"]),
            LatestPrompt::Unknown
        );
    }

    /// The bridge leaves local-only slash commands out of its replay, and
    /// the SDK replays prompt commands expanded: a newest prompt that is a
    /// slash command can't be told missing from the replay lacking it.
    #[test]
    fn a_newest_slash_command_the_replay_lacks_is_unknown() {
        for command in ["/compact", "/context", "/model opus", "/review the diff"] {
            let stored = [prompt("Fix the build"), prompt(command)];
            assert_eq!(
                latest_prompt_after(&stored, &["Fix the build"]),
                LatestPrompt::Unknown,
                "{command}"
            );
        }
        // Replayed as typed (a custom command), it was received.
        let stored = [prompt("Fix the build"), prompt("/review the diff")];
        assert_eq!(
            latest_prompt_after(&stored, &["Fix the build", "/review the diff"]),
            LatestPrompt::Replayed
        );
    }

    /// `Missing` needs the replay to end on the prompt before the newest. A
    /// replay that ends on something else may hold the newest prompt in a
    /// form its stored text doesn't match.
    #[test]
    fn missing_needs_the_replay_to_end_on_the_prompt_before_the_newest() {
        let stored = [
            prompt("Fix the build"),
            prompt("Now add a test"),
            prompt("Ship it"),
        ];
        assert_eq!(
            latest_prompt_after(
                &stored,
                &["Fix the build", "Now add a test", "Ship it, please"]
            ),
            LatestPrompt::Replayed
        );
        assert_eq!(
            latest_prompt_after(
                &stored,
                &["Fix the build", "Now add a test", "Ship this to staging"]
            ),
            LatestPrompt::Unknown,
            "the newest prompt may have been re-worded"
        );
        assert_eq!(
            latest_prompt_after(&stored, &["Fix the build", "Now add a test"]),
            LatestPrompt::Missing
        );
    }

    #[test]
    fn a_prompt_replayed_in_chunks_is_one_prompt() {
        let stored = [prompt("Look at this screenshot and fix it")];
        let mut ledger = replaying(&stored);
        ledger.action_for(&user_text(None, "Look at this screenshot "));
        ledger.action_for(&user_text(None, "and fix it"));
        ledger.go_live();
        assert_eq!(ledger.latest_prompt(), LatestPrompt::Replayed);
    }

    #[test]
    fn nothing_to_tell_from_is_unknown() {
        // The agent replays no prompts.
        let stored = [prompt("Fix the build")];
        let mut ledger = replaying(&stored);
        ledger.action_for(&text(Some("a0"), "Fixed."));
        ledger.go_live();
        assert_eq!(ledger.latest_prompt(), LatestPrompt::Unknown);
        // The transcript holds no prompt text (an image-only prompt).
        assert_eq!(
            latest_prompt_after(&[prompt("  ")], &["something"]),
            LatestPrompt::Unknown
        );
        assert_eq!(
            latest_prompt_after(&[], &["something"]),
            LatestPrompt::Unknown
        );
    }

    /// A steer's echo is recorded with the agent's message id: it is the
    /// agent's copy of a prompt Staged stored, not a prompt of its own.
    #[test]
    fn recorded_prompt_echoes_do_not_count_as_prompts() {
        let stored = [
            prompt("Fix the build"),
            boundary("user", "Fix the build", Some("echo"), None),
        ];
        assert_eq!(
            latest_prompt_after(&stored, &["Fix the build"]),
            LatestPrompt::Replayed
        );
    }
}
