//! Output of executions that have ended.
//!
//! A viewer can still ask for an execution's output shortly after it ends, so
//! the executor cannot drop the buffer at exit. It keeps the buffers of the
//! last few executions instead, which bounds what finished work costs however
//! long the process lives.

use std::collections::VecDeque;

use crate::output_buffer::SharedOutputBuffer;

/// How many finished executions keep their output.
pub(crate) const MAX_FINISHED_EXECUTIONS: usize = 8;

/// The output buffers of the most recently finished executions.
pub(crate) struct FinishedOutputs {
    /// Oldest first.
    entries: VecDeque<(String, SharedOutputBuffer)>,
    max_entries: usize,
}

impl FinishedOutputs {
    pub(crate) fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries,
        }
    }

    /// Retain the output of an execution that has just ended, evicting the
    /// longest-finished ones beyond the limit.
    pub(crate) fn insert(&mut self, execution_id: String, output: SharedOutputBuffer) {
        self.entries.push_back((execution_id, output));
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
    }

    pub(crate) fn get(&self, execution_id: &str) -> Option<SharedOutputBuffer> {
        self.entries
            .iter()
            .find(|(id, _)| id == execution_id)
            .map(|(_, output)| output.clone())
    }

    /// Stop retaining an execution's output. Returns whether it was retained.
    pub(crate) fn remove(&mut self, execution_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(id, _)| id != execution_id);
        self.entries.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output_buffer::OutputBuffer;

    fn insert(finished: &mut FinishedOutputs, id: &str) {
        finished.insert(id.to_string(), OutputBuffer::shared(1024));
    }

    #[test]
    fn the_longest_finished_execution_is_evicted_first() {
        let mut finished = FinishedOutputs::new(2);
        insert(&mut finished, "a");
        insert(&mut finished, "b");
        assert!(finished.get("a").is_some());

        insert(&mut finished, "c");
        assert!(finished.get("a").is_none());
        assert!(finished.get("b").is_some());
        assert!(finished.get("c").is_some());
    }

    #[test]
    fn remove_reports_whether_the_execution_was_retained() {
        let mut finished = FinishedOutputs::new(2);
        insert(&mut finished, "a");

        assert!(finished.remove("a"));
        assert!(finished.get("a").is_none());
        assert!(!finished.remove("a"));
    }

    #[test]
    fn removing_one_execution_frees_its_slot() {
        let mut finished = FinishedOutputs::new(2);
        insert(&mut finished, "a");
        insert(&mut finished, "b");
        finished.remove("a");
        insert(&mut finished, "c");

        assert!(finished.get("b").is_some());
        assert!(finished.get("c").is_some());
    }

    #[test]
    fn a_limit_of_zero_retains_nothing() {
        let mut finished = FinishedOutputs::new(0);
        insert(&mut finished, "a");
        assert!(finished.get("a").is_none());
    }
}
