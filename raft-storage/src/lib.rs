use anyhow::Result;
use async_trait::async_trait;
use raft_core::Storage;
use std::sync::Arc;
use tokio::sync::Mutex;
use wcygan_raft_community_neoeinstein_prost::raft::v1::{
    HardState as ProstHardState, LogEntry as ProstLogEntry,
};

/// Internal state for InMemoryStorage, protected by a Mutex.
#[derive(Debug, Clone, Default)]
struct InMemoryStorageInner {
    hard_state: ProstHardState,
    log: Vec<ProstLogEntry>,
    // Optionally store snapshot data if implementing snapshotting later
    // snapshot: Option<(Vec<u8>, u64, u64)>,
}

/// An in-memory implementation of the `Storage` trait for Raft.
/// Suitable for testing and simple use cases.
#[derive(Debug, Clone, Default)]
pub struct InMemoryStorage {
    inner: Arc<Mutex<InMemoryStorageInner>>,
}

impl InMemoryStorage {
    /// Creates a new, empty `InMemoryStorage`.
    pub fn new() -> Self {
        Default::default()
    }
}

#[async_trait]
impl Storage for InMemoryStorage {
    async fn save_hard_state(&mut self, state: &ProstHardState) -> Result<()> {
        let mut inner = self.inner.lock().await;
        tracing::debug!(
            new_term = state.term,
            new_voted_for = state.voted_for,
            new_commit = state.commit_index,
            "Saving HardState"
        );
        inner.hard_state = *state;
        Ok(())
    }

    async fn read_hard_state(&self) -> Result<ProstHardState> {
        let inner = self.inner.lock().await;
        tracing::debug!(
            term = inner.hard_state.term,
            voted_for = inner.hard_state.voted_for,
            commit = inner.hard_state.commit_index,
            "Reading HardState"
        );
        Ok(inner.hard_state)
    }

    async fn append_log_entries(&mut self, entries: &[ProstLogEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let first_new_index = entries.first().map(|e| e.index).unwrap_or(0);
        let last_new_index = entries.last().map(|e| e.index).unwrap_or(0);
        tracing::debug!(
            count = entries.len(),
            first_index = first_new_index,
            last_index = last_new_index,
            "Appending log entries"
        );

        // Basic validation: Ensure incoming entries have consecutive indices
        // More robust validation (term checks) should happen in the core Raft logic
        for i in 1..entries.len() {
            if entries[i].index != entries[i - 1].index + 1 {
                // This indicates a potential gap or overlap, which shouldn't happen
                // if the Raft logic correctly prepares the entries slice.
                // We could panic or return an error, but tracing might be sufficient for now.
                tracing::error!(
                    prev_index = entries[i - 1].index,
                    current_index = entries[i].index,
                    "Non-consecutive log entry indices detected during append"
                );
                // Depending on strictness, might return Err(...) here.
            }
        }

        // Check if the new entries overwrite existing ones
        if let Some(first_entry) = entries.first() {
            let log_len = inner.log.len() as u64;
            // Raft indices are 1-based
            let start_index_0based = (first_entry.index.saturating_sub(1)) as usize;

            if first_entry.index <= log_len {
                // Overwriting existing entries
                tracing::warn!(
                    overwrite_start_index = first_entry.index,
                    current_log_len = log_len,
                    "Overwriting existing log entries"
                );
                // Truncate the existing log from the point of the first new entry
                inner.log.truncate(start_index_0based);
            } else if first_entry.index > log_len + 1 {
                // Gap detected! The Raft protocol should prevent this.
                tracing::error!(
                    first_new_index = first_entry.index,
                    log_len = log_len,
                    "Gap detected between existing log and new entries"
                );
                // Return an error or handle as appropriate for Raft invariants
                return Err(anyhow::anyhow!(
                    "Gap detected between log end ({}) and new entries starting at {}",
                    log_len,
                    first_entry.index
                ));
            }
        }

        inner.log.extend_from_slice(entries);
        Ok(())
    }

    async fn read_log_entry(&self, index: u64) -> Result<Option<ProstLogEntry>> {
        let inner = self.inner.lock().await;
        // Log indices are 1-based
        if index == 0 {
            return Ok(None);
        }
        // Find the entry with the matching absolute index.
        // This correctly handles cases where the log prefix has been truncated.
        let entry = inner
            .log
            .iter()
            .find(|&entry| entry.index == index)
            .cloned();

        tracing::trace!(index, found = entry.is_some(), "Read log entry");
        Ok(entry)
    }

    async fn read_log_entries(
        &self,
        start_index: u64, // Inclusive, 1-based
        end_index: u64,   // Exclusive, 1-based
    ) -> Result<Vec<ProstLogEntry>> {
        let inner = self.inner.lock().await;

        tracing::trace!(
            start_index,
            end_index,
            log_len = inner.log.len(),
            first_entry_index = inner.log.first().map(|e| e.index),
            last_entry_index = inner.log.last().map(|e| e.index),
            "Reading log entries range"
        );

        if start_index >= end_index || inner.log.is_empty() {
            return Ok(Vec::new());
        }

        // Iterate and filter based on absolute index.
        let result: Vec<ProstLogEntry> = inner
            .log
            .iter()
            .filter(|&entry| entry.index >= start_index && entry.index < end_index)
            .cloned()
            .collect();

        tracing::trace!(count = result.len(), "Returning log entries");
        Ok(result)
    }

    async fn truncate_log_prefix(&mut self, end_index_exclusive: u64) -> Result<()> {
        let mut inner = self.inner.lock().await;
        tracing::debug!(
            end_index_exclusive,
            current_len = inner.log.len(),
            first_index = inner.log.first().map(|e| e.index),
            last_index = inner.log.last().map(|e| e.index),
            "Truncating log prefix (keeping >= index {})",
            end_index_exclusive
        );

        if end_index_exclusive <= 1 {
            // Treat 0 or 1 as keeping everything (truncating nothing before index 1).
            tracing::debug!("Truncate index <= 1, keeping all entries.");
            return Ok(());
        }

        // Find the position (Vec index) of the first entry to keep.
        // We keep entries with absolute index >= end_index_exclusive.
        let keep_from_pos = inner
            .log
            .iter()
            .position(|entry| entry.index >= end_index_exclusive);

        match keep_from_pos {
            Some(pos) => {
                // Found the first entry to keep at Vec index `pos`.
                // Drain all elements before this position.
                tracing::debug!(
                    "Found first entry to keep (index {}) at Vec position {}, draining prefix.",
                    inner.log[pos].index,
                    pos
                );
                inner.log.drain(..pos);
            }
            None => {
                // No entry has index >= end_index_exclusive.
                // This means we should remove *all* entries.
                tracing::debug!(
                    "No entry found with index >= {}, clearing log.",
                    end_index_exclusive
                );
                inner.log.clear();
            }
        }

        Ok(())
    }

    async fn truncate_log_suffix(&mut self, start_index_inclusive: u64) -> Result<()> {
        let mut inner = self.inner.lock().await;
        // Indices are 1-based, inclusive start
        // We want to keep entries UP TO (but not including) start_index_inclusive.
        tracing::debug!(
            start_index_inclusive,
            current_len = inner.log.len(),
            "Truncating log suffix"
        );
        if start_index_inclusive == 0 {
            // Not a valid 1-based index, interpret as truncating everything? Or error?
            // Let's treat 0 or 1 as truncating everything *before* index 1 (i.e., keep nothing).
            tracing::warn!("Truncate suffix called with index 0, clearing log.");
            inner.log.clear();
            return Ok(());
        }

        let truncate_at_0based = (start_index_inclusive.saturating_sub(1)) as usize;
        if truncate_at_0based < inner.log.len() {
            inner.log.truncate(truncate_at_0based);
        }
        // If truncate_at_0based >= len, truncate does nothing, which is correct.
        Ok(())
    }

    async fn last_log_index(&self) -> Result<u64> {
        let inner = self.inner.lock().await;
        // Return the absolute index of the last entry, or 0 if the log is empty.
        let index = inner.log.last().map_or(0, |entry| entry.index);
        tracing::trace!(last_index = index, "Getting last log index");
        Ok(index)
    }

    async fn last_log_term(&self) -> Result<u64> {
        let inner = self.inner.lock().await;
        let term = inner.log.last().map_or(0, |entry| entry.term); // Term is 0 if log is empty
        tracing::trace!(last_term = term, "Getting last log term");
        Ok(term)
    }

    // TODO: Implement snapshot methods if/when needed based on the plan
    // async fn save_snapshot(&mut self, snapshot: &[u8], last_included_index: u64, last_included_term: u64) -> Result<()>;
    // async fn read_snapshot(&self) -> Result<Option<(Vec<u8>, u64, u64)>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn create_entry(index: u64, term: u64) -> ProstLogEntry {
        ProstLogEntry {
            index,
            term,
            command: Bytes::from(format!("cmd{}", index)),
        }
    }

    #[tokio::test]
    async fn test_hard_state_read_write() {
        let mut storage = InMemoryStorage::new();
        let initial_state = storage.read_hard_state().await.unwrap();
        assert_eq!(
            initial_state,
            ProstHardState::default(),
            "Initial state should be default"
        );

        let new_state = ProstHardState {
            term: 5,
            voted_for: 3,
            commit_index: 10,
        };
        storage.save_hard_state(&new_state).await.unwrap();

        let read_state = storage.read_hard_state().await.unwrap();
        assert_eq!(read_state, new_state, "Read state should match saved state");

        let newer_state = ProstHardState {
            term: 6,
            ..new_state
        };
        storage.save_hard_state(&newer_state).await.unwrap();
        let read_newer_state = storage.read_hard_state().await.unwrap();
        assert_eq!(
            read_newer_state, newer_state,
            "Read state should reflect the latest save"
        );
    }

    #[tokio::test]
    async fn test_log_append_and_read_single() {
        let mut storage = InMemoryStorage::new();
        let entry1 = create_entry(1, 1);

        storage.append_log_entries(&[entry1.clone()]).await.unwrap();

        assert_eq!(storage.last_log_index().await.unwrap(), 1);
        assert_eq!(storage.last_log_term().await.unwrap(), 1);

        let read_entry = storage.read_log_entry(1).await.unwrap();
        assert_eq!(
            read_entry,
            Some(entry1.clone()),
            "Should read back the appended entry"
        );

        let read_nonexistent = storage.read_log_entry(0).await.unwrap();
        assert!(read_nonexistent.is_none(), "Index 0 should return None");
        let read_nonexistent = storage.read_log_entry(2).await.unwrap();
        assert!(
            read_nonexistent.is_none(),
            "Index beyond log end should return None"
        );
    }

    #[tokio::test]
    async fn test_log_append_multiple_and_read_range() {
        let mut storage = InMemoryStorage::new();
        let entries = vec![
            create_entry(1, 1),
            create_entry(2, 1),
            create_entry(3, 2),
            create_entry(4, 2),
        ];

        storage.append_log_entries(&entries).await.unwrap();

        assert_eq!(storage.last_log_index().await.unwrap(), 4);
        assert_eq!(storage.last_log_term().await.unwrap(), 2);

        // Read entire log
        let read_entries = storage.read_log_entries(1, 5).await.unwrap(); // End index is exclusive
        assert_eq!(
            read_entries, entries,
            "Should read back all appended entries"
        );

        // Read a sub-range
        let read_sub = storage.read_log_entries(2, 4).await.unwrap(); // Entries at index 2 and 3
        assert_eq!(read_sub, &entries[1..3], "Should read back sub-range");

        // Read range starting from 1
        let read_start = storage.read_log_entries(1, 3).await.unwrap(); // Entries at index 1 and 2
        assert_eq!(read_start, &entries[0..2], "Should read range from start");

        // Read range ending at last element
        let read_end = storage.read_log_entries(3, 5).await.unwrap(); // Entries at index 3 and 4
        assert_eq!(read_end, &entries[2..4], "Should read range to end");

        // Read empty range (start == end)
        let read_empty1 = storage.read_log_entries(2, 2).await.unwrap();
        assert!(read_empty1.is_empty(), "Start == end should yield empty");

        // Read empty range (start > end)
        let read_empty2 = storage.read_log_entries(3, 2).await.unwrap();
        assert!(read_empty2.is_empty(), "Start > end should yield empty");

        // Read range beyond log end
        let read_beyond = storage.read_log_entries(4, 10).await.unwrap(); // Entry at index 4
        assert_eq!(read_beyond, &entries[3..], "Should read up to log end");

        // Read range starting beyond log end
        let read_start_beyond = storage.read_log_entries(5, 10).await.unwrap();
        assert!(
            read_start_beyond.is_empty(),
            "Start beyond end should yield empty"
        );

        // Read range with start=0 (should be treated as 1)
        let read_zero_start = storage.read_log_entries(0, 3).await.unwrap();
        assert_eq!(
            read_zero_start,
            &entries[0..2],
            "Start=0 should act like start=1"
        );
    }

    #[tokio::test]
    async fn test_append_overwrite() {
        let mut storage = InMemoryStorage::new();
        let initial_entries = vec![create_entry(1, 1), create_entry(2, 1), create_entry(3, 1)];
        storage.append_log_entries(&initial_entries).await.unwrap();

        let overwrite_entries = vec![create_entry(2, 2), create_entry(3, 2)]; // Different term for index 2,3
        storage
            .append_log_entries(&overwrite_entries)
            .await
            .unwrap();

        assert_eq!(
            storage.last_log_index().await.unwrap(),
            3,
            "Last index should still be 3 after overwrite"
        );
        assert_eq!(
            storage.last_log_term().await.unwrap(),
            2,
            "Last term should be from the overwrite"
        );

        let entry1 = storage.read_log_entry(1).await.unwrap().unwrap();
        let entry2 = storage.read_log_entry(2).await.unwrap().unwrap();
        let entry3 = storage.read_log_entry(3).await.unwrap().unwrap();

        assert_eq!(entry1, initial_entries[0], "Entry 1 should be unchanged");
        assert_eq!(
            entry2, overwrite_entries[0],
            "Entry 2 should be overwritten"
        );
        assert_eq!(
            entry3, overwrite_entries[1],
            "Entry 3 should be overwritten"
        );

        let all_entries = storage.read_log_entries(1, 4).await.unwrap();
        assert_eq!(all_entries.len(), 3);
        assert_eq!(all_entries[0].term, 1);
        assert_eq!(all_entries[1].term, 2);
        assert_eq!(all_entries[2].term, 2);
    }

    #[tokio::test]
    async fn test_append_gap_error() {
        let mut storage = InMemoryStorage::new();
        storage
            .append_log_entries(&[create_entry(1, 1)])
            .await
            .unwrap();

        // Attempt to append starting at index 3 (gap)
        let gap_entries = vec![create_entry(3, 2)];
        let result = storage.append_log_entries(&gap_entries).await;

        assert!(
            result.is_err(),
            "Appending with a gap should return an error"
        );
        assert_eq!(
            storage.last_log_index().await.unwrap(),
            1,
            "Log should remain unchanged after gap error"
        );
    }

    #[tokio::test]
    async fn test_truncate_suffix() {
        let mut storage = InMemoryStorage::new();
        let entries = vec![
            create_entry(1, 1),
            create_entry(2, 1),
            create_entry(3, 2),
            create_entry(4, 2),
            create_entry(5, 3),
        ];
        storage.append_log_entries(&entries).await.unwrap();

        // Truncate entries from index 4 onwards (keep 1, 2, 3)
        storage.truncate_log_suffix(4).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 3);
        assert_eq!(storage.last_log_term().await.unwrap(), 2);
        assert!(storage.read_log_entry(4).await.unwrap().is_none());
        assert!(storage.read_log_entry(5).await.unwrap().is_none());
        assert_eq!(storage.read_log_entry(3).await.unwrap().unwrap().term, 2);

        // Truncate from index 1 (keep nothing)
        storage.truncate_log_suffix(1).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
        assert_eq!(storage.last_log_term().await.unwrap(), 0);
        assert!(storage.read_log_entry(1).await.unwrap().is_none());

        // Test truncating an already empty log
        storage.truncate_log_suffix(1).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 0);

        // Test truncating beyond the end (should do nothing)
        storage.append_log_entries(&entries[0..2]).await.unwrap(); // Add back 1, 2
        assert_eq!(storage.last_log_index().await.unwrap(), 2);
        storage.truncate_log_suffix(10).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 2);

        // Test truncating with index 0 (should clear log)
        storage.truncate_log_suffix(0).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
        assert!(storage.read_log_entry(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_truncate_prefix() {
        let mut storage = InMemoryStorage::new();
        let entries = vec![
            create_entry(1, 1),
            create_entry(2, 1),
            create_entry(3, 2),
            create_entry(4, 2),
            create_entry(5, 3),
        ];
        storage.append_log_entries(&entries).await.unwrap();

        // Truncate entries before index 3 (keep 3, 4, 5)
        storage.truncate_log_prefix(3).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 5);
        assert_eq!(storage.last_log_term().await.unwrap(), 3);
        // These should now correctly find nothing because entries 1 and 2 are gone.
        assert!(
            storage.read_log_entry(1).await.unwrap().is_none(),
            "Entry 1 should be gone after truncate_prefix(3)"
        );
        assert!(
            storage.read_log_entry(2).await.unwrap().is_none(),
            "Entry 2 should be gone after truncate_prefix(3)"
        );

        // Check remaining entries by absolute index
        let entry3 = storage.read_log_entry(3).await.unwrap().unwrap();
        assert_eq!(entry3.term, 2, "Entry 3 should remain");
        let entry5 = storage.read_log_entry(5).await.unwrap().unwrap();
        assert_eq!(entry5.term, 3, "Entry 5 should remain");

        // Read range should now work correctly across the truncated gap
        let remaining_entries = storage.read_log_entries(1, 6).await.unwrap(); // Ask for 1..6
        assert_eq!(
            remaining_entries.len(),
            3,
            "Should only get entries 3, 4, 5"
        );
        assert_eq!(remaining_entries[0].index, 3);
        assert_eq!(remaining_entries[2].index, 5);

        // Truncate before index 5 (keep 5 onwards)
        storage.truncate_log_prefix(5).await.unwrap(); // Keeps only entry 5
        assert_eq!(storage.last_log_index().await.unwrap(), 5);
        assert!(storage.read_log_entry(4).await.unwrap().is_none());
        assert!(storage.read_log_entry(5).await.unwrap().is_some());
        assert_eq!(
            storage.read_log_entries(1, 6).await.unwrap().len(),
            1,
            "Only entry 5 should remain"
        );

        // Truncate before index 1 (keep all)
        storage = InMemoryStorage::new();
        storage.append_log_entries(&entries).await.unwrap();
        storage.truncate_log_prefix(1).await.unwrap(); // Keeps [1..5]
        assert_eq!(storage.last_log_index().await.unwrap(), 5);
        assert!(storage.read_log_entry(1).await.unwrap().is_some());
        assert_eq!(storage.read_log_entries(1, 6).await.unwrap().len(), 5);

        // Truncate before index 7 (way past end -> clear all)
        storage.truncate_log_prefix(7).await.unwrap(); // Keeps >= 7. Removes 1..5.
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
        assert!(storage.read_log_entry(5).await.unwrap().is_none());
        assert_eq!(storage.inner.lock().await.log.len(), 0);
        assert!(storage.read_log_entries(1, 6).await.unwrap().is_empty());

        // Truncate empty log
        storage = InMemoryStorage::new();
        storage.truncate_log_prefix(1).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
        storage.truncate_log_prefix(10).await.unwrap();
        assert_eq!(storage.last_log_index().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_truncate_prefix_minimal_no_op() {
        let mut storage = InMemoryStorage::new();
        let entry1 = create_entry(1, 1);
        storage.append_log_entries(&[entry1.clone()]).await.unwrap();

        // Truncate before index 1 (should remove nothing)
        storage.truncate_log_prefix(1).await.unwrap();

        assert_eq!(
            storage.last_log_index().await.unwrap(),
            1,
            "Last index should still be 1"
        );

        let entries_read = storage.read_log_entries(1, 2).await.unwrap(); // Read index 1
        assert_eq!(
            entries_read.len(),
            1,
            "Should read back 1 entry after no-op truncate"
        );
        assert_eq!(entries_read[0], entry1);
    }

    #[tokio::test]
    async fn test_last_index_term_empty_log() {
        let storage = InMemoryStorage::new();
        assert_eq!(
            storage.last_log_index().await.unwrap(),
            0,
            "Last index of empty log is 0"
        );
        assert_eq!(
            storage.last_log_term().await.unwrap(),
            0,
            "Last term of empty log is 0"
        );
    }

    #[tokio::test]
    async fn test_log_reappend_same_index() {
        // Scenario: Append [1, 2], then append [2, 3] -> should result in [1, 2, 3]
        let mut storage = InMemoryStorage::new();
        let entries1 = vec![create_entry(1, 1), create_entry(2, 1)];
        storage.append_log_entries(&entries1).await.unwrap();

        let entries2 = vec![create_entry(2, 1), create_entry(3, 1)]; // Overwrite index 2, append index 3
        storage.append_log_entries(&entries2).await.unwrap();

        assert_eq!(storage.last_log_index().await.unwrap(), 3);
        assert_eq!(storage.last_log_term().await.unwrap(), 1);

        let final_log = storage.read_log_entries(1, 4).await.unwrap();
        assert_eq!(final_log.len(), 3);
        assert_eq!(final_log[0].index, 1);
        assert_eq!(final_log[1].index, 2); // Should be from entries2
        assert_eq!(final_log[2].index, 3);
    }
}
