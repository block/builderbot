//! Catch-up for an attach: recording what a `session/load` replay, and the
//! live stream after it, add to a transcript that already holds part of the
//! session.
//!
//! The replay re-sends the whole session. Everything the transcript already
//! holds is dropped by identity: text by its ACP message id (or, for an agent
//! that sends none, by position in the stored text), tool calls and their
//! results by tool-call id. The replay can end mid-turn — its last tool call
//! may be `pending`, with the result still to come live — so nothing waits for
//! the replay to "complete"; every frame stands on its own.
//!
//! The replay also says whether the agent ever received the newest prompt the
//! transcript holds ([`LatestPrompt`]): Staged persists a prompt before it
//! sends it, so a connection lost in between leaves a prompt in the transcript
//! that the agent's session does not have.
use super::*;

/// Which part of an attach a frame arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatchUpStage {
    /// Before `session/load`: the in-flight turn's frames the proxy relayed
    /// as soon as it attached, which the own-session gate held until the
    /// session was named. The replay re-sends everything they describe, in
    /// transcript order, so they are dropped: recorded here they would land
    /// ahead of the older turns the replay is about to backfill. The one
    /// exception is text of a message still streaming, which an agent that
    /// writes messages to its transcript only once complete (the Claude
    /// bridge) cannot replay: that is kept, and recorded ahead of the
    /// message's next live chunk, or once the replay is over (see
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
    /// The replay re-sent prompts, but not this one: the agent never received
    /// it (the connection went between Staged persisting the prompt and
    /// sending it). Whatever turn the session shows ended is an earlier one.
    Missing,
    /// Nothing to tell from: the transcript holds no prompt text, or the
    /// agent replays no prompts at all.
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
            std::cmp::Ordering::Less => LatestPrompt::Missing,
            std::cmp::Ordering::Equal => LatestPrompt::Unknown,
        }
    }
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
    /// Tool calls whose result the transcript holds, or this attach recorded.
    tool_results: HashSet<String>,
    /// The replay has produced something the transcript did not hold, so the
    /// events it carries from here on (usage, plans) are new too.
    past_boundary: bool,
    /// Held text per message id, and the order the messages first appeared.
    /// An entry goes once the replay stage shows what its message is.
    held: HashMap<String, HeldMessage>,
    held_order: Vec<String>,
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
        let mut tool_results = HashSet::new();
        for boundary in stored {
            match (boundary.role.as_str(), &boundary.acp_tool_call_id) {
                ("tool_call", Some(id)) => {
                    tool_calls.insert(id.clone());
                }
                ("tool_result", Some(id)) => {
                    tool_results.insert(id.clone());
                }
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
            tool_results,
            past_boundary: false,
            held: HashMap::new(),
            held_order: Vec::new(),
            streaming: HashSet::new(),
            prompts: PromptLedger::new(stored),
        }
    }

    pub(super) fn set_stage(&mut self, stage: CatchUpStage) {
        self.stage = stage;
    }

    /// The replay is over; from here every frame is the turn's live stream.
    /// Returns the text to record before the next live frame: the held text
    /// of each message the replay stage carried nothing for, in the order
    /// the messages first appeared.
    pub(super) fn go_live(&mut self) -> Vec<SessionUpdate> {
        self.prompts.close();
        self.stage = CatchUpStage::Live;
        let mut held = std::mem::take(&mut self.held);
        std::mem::take(&mut self.held_order)
            .into_iter()
            .filter_map(|id| held.remove(&id))
            .filter(|message| !message.text.is_empty())
            .map(|message| {
                SessionUpdate::AgentMessageChunk(with_text(&message.first, &message.text))
            })
            .collect()
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
        if self.stage == CatchUpStage::Held {
            self.hold(update);
            return LiveAction::Drop;
        }
        if self.stage == CatchUpStage::Replay {
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
        }
        match update {
            SessionUpdate::ToolCall(tool_call) => {
                if !self.tool_calls.insert(tool_call.tool_call_id.0.to_string()) {
                    return LiveAction::Drop;
                }
                self.past_boundary = true;
                live_action_for_update(update)
            }
            SessionUpdate::ToolCallUpdate(tool_update) => {
                let id = tool_update.tool_call_id.0.to_string();
                if self.tool_results.contains(&id) {
                    return LiveAction::Drop;
                }
                let action = live_action_for_update(update);
                if let LiveAction::ToolCallUpdate {
                    result: Some(_), ..
                } = &action
                {
                    self.tool_results.insert(id);
                    self.past_boundary = true;
                }
                action
            }
            SessionUpdate::AgentMessageChunk(chunk) if self.stage == CatchUpStage::Replay => {
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
            SessionUpdate::UserMessageChunk(_) if self.stage == CatchUpStage::Replay => {
                LiveAction::Drop
            }
            _ if self.stage == CatchUpStage::Replay => self.past_boundary_event(update),
            _ => live_action_for_update(update),
        }
    }

    /// Keep a held text chunk that names its message, in case the replay
    /// never re-sends that message.
    fn hold(&mut self, update: &SessionUpdate) {
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
        ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
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
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new(id),
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec!["done".to_string().into()]),
        ))
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
        assert!(matches!(
            ledger.action_for(&tool_call("t3")),
            LiveAction::Drop
        ));
        assert!(matches!(
            ledger.action_for(&tool_result("t3")),
            LiveAction::ToolCallUpdate {
                result: Some(_),
                ..
            }
        ));
        assert!(matches!(
            ledger.action_for(&tool_result("t3")),
            LiveAction::Drop
        ));
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
    /// markers) are skipped.
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
        assert_eq!(
            latest_prompt_after(&stored, &["Fix the build", "[Request interrupted by user]"]),
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
