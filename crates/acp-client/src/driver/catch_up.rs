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
use super::*;

/// Which part of an attach a frame arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatchUpStage {
    /// Before `session/load`: the in-flight turn's frames the proxy relayed
    /// as soon as it attached, which the own-session gate held until the
    /// session was named. The replay re-sends everything they describe, in
    /// transcript order, so they are dropped: recorded here they would land
    /// ahead of the older turns the replay is about to backfill.
    Held,
    /// `session/load` is out: the replay.
    Replay,
    /// The load answered: the turn's live stream.
    Live,
}

/// Text the transcript already holds, and how far the replay has walked it.
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
    /// new text, and so is everything after it.
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

/// An attach's catch-up state (see the module docs).
pub(super) struct CatchUpLedger {
    stage: CatchUpStage,
    /// Stored text per ACP message id: user and assistant alike, since both
    /// replay as message chunks.
    messages: HashMap<String, StoredText>,
    /// Assistant text stored without a message id, in transcript order, as
    /// one stream: an agent that sends no ids is matched by position.
    unidentified: StoredText,
    /// Tool calls the transcript holds, or this attach recorded.
    tool_calls: HashSet<String>,
    /// Tool calls whose result the transcript holds, or this attach recorded.
    tool_results: HashSet<String>,
    /// The replay has produced something the transcript did not hold, so the
    /// events it carries from here on (usage, plans) are new too.
    past_boundary: bool,
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
            unidentified: StoredText::new(unidentified),
            tool_calls,
            tool_results,
            past_boundary: false,
        }
    }

    pub(super) fn set_stage(&mut self, stage: CatchUpStage) {
        self.stage = stage;
    }

    /// Whether a `result` frame may close a response: not one held from
    /// before the load, which would be recorded ahead of the replay.
    pub(super) fn accepts_results(&self) -> bool {
        self.stage != CatchUpStage::Held
    }

    /// What `update` records.
    pub(super) fn action_for(&mut self, update: &SessionUpdate) -> LiveAction {
        if self.stage == CatchUpStage::Held {
            return LiveAction::Drop;
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
                let stored = match &chunk.message_id {
                    Some(id) => self.messages.entry(id.0.to_string()).or_default(),
                    None => &mut self.unidentified,
                };
                let Some(unseen) = stored.unseen(&text.text) else {
                    return LiveAction::Drop;
                };
                self.past_boundary = true;
                let mut chunk = chunk.clone();
                chunk.content = AcpContentBlock::Text(TextContent::new(unseen));
                LiveAction::AppendText {
                    text: unseen.to_string(),
                    metadata: message_chunk_metadata("agent_message_chunk", &chunk),
                }
            }
            // The replayed prompts are the transcript's own user rows.
            SessionUpdate::UserMessageChunk(_) if self.stage == CatchUpStage::Replay => {
                LiveAction::Drop
            }
            _ if self.stage == CatchUpStage::Replay => self.past_boundary_event(update),
            _ => live_action_for_update(update),
        }
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

    fn text(message_id: Option<&str>, text: &str) -> SessionUpdate {
        let mut chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(text)));
        chunk.message_id = message_id.map(|id| id.to_string().into());
        SessionUpdate::AgentMessageChunk(chunk)
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
            appended(ledger.action_for(&text(None, "second, then more"))),
            Some("second, then more".to_string()),
            "text that does not line up is new"
        );
        assert_eq!(
            appended(ledger.action_for(&text(None, "first"))),
            Some("first".to_string()),
            "and so is everything after it"
        );
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
            ledger.action_for(&text(Some("m9"), "x")),
            LiveAction::Drop
        ));
        assert!(matches!(
            ledger.action_for(&tool_call("t9")),
            LiveAction::Drop
        ));
        assert!(!ledger.accepts_results());
        ledger.set_stage(CatchUpStage::Live);
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

    #[test]
    fn replayed_prompts_and_events_before_the_boundary_are_dropped() {
        let mut ledger = replaying(&[boundary("assistant", "a", Some("m1"), None)]);
        let mut prompt = ContentChunk::new(AcpContentBlock::Text(TextContent::new("hi")));
        prompt.message_id = Some("u1".to_string().into());
        assert!(matches!(
            ledger.action_for(&SessionUpdate::UserMessageChunk(prompt)),
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
}
