//! Byte-addressed copy-on-write write transaction (S9-11) over
//! [`CowFile::commit`](crate::cow_proto::CowFile::commit).
//!
//! **Prototype, not a normative format.** The transaction collects *dirty*
//! logical blocks in memory and publishes them with a single commit:
//!
//! * [`CowTx::write_at`] takes a byte range. A range that covers a block only
//!   partly is a read-modify-write: the old block is read through the
//!   published mapping, modified in memory and committed as a **new** physical
//!   block; the old block is never touched. A range that covers a whole block
//!   does not read it. A range past the end extends the disk; the gap is
//!   zero-filled and counts as dirty.
//! * Dirty state is bounded by [`MAX_TX_BLOCKS`]. A `write_at` that would
//!   exceed it fails with [`TxError::TooLarge`] *before any backend call* and
//!   leaves the transaction unchanged.
//! * [`CowTx::view`] is the reader view: the published mapping the transaction
//!   started from. It cannot change while the transaction is open.
//!   [`CowTx::commit`] publishes every dirty block together, so a reader after
//!   a successful commit sees the whole new mapping.
//!
//! # Outcomes
//!
//! | event | result | published state | session |
//! |---|---|---|---|
//! | [`cancel`](CowTx::cancel) before commit | `Ok` | old, backend never called | usable |
//! | cancel hook says yes before barrier 1 | [`TxError::Cancelled`] | old | usable |
//! | commit returns `Ok(ack)` | published and durable | new | usable |
//! | `cancel` after that `Ok` | [`TxError::AlreadyCommitted`] | new | usable |
//! | any I/O failure in commit | [`TxError::Uncertain`] | old **or** new | poisoned |
//!
//! After `Uncertain` nothing may be retried on the same file: every further
//! call is [`TxError::ReconcileRequired`] without touching the backend. The
//! only way forward is to drop the file and reopen the media, which decides
//! the state (see `cow_proto`, "Recovery and root selection").

use crate::cow_proto::{
    BLOCK_SIZE, CommitStep, CowError, CowFile, CowView, MAX_BLOCKS, MAX_TX_BLOCKS,
};
use crate::io::{DurableAck, StorageBackend};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxState {
    Open,
    Committed,
    Cancelled,
    /// A commit failed after I/O started; the outcome is unknown.
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TxError {
    /// Rejected without changing anything (bad input, a failed read of an old
    /// block, an empty transaction). The transaction stays open.
    Rejected(CowError),
    /// The write would make more than `limit` dirty blocks.
    TooLarge { limit: usize, needed: usize },
    /// Offset arithmetic overflowed or the disk would exceed `MAX_BLOCKS`.
    OutOfRange,
    /// The transaction already ended (committed or cancelled).
    NotOpen(TxState),
    /// `cancel` after the commit was published.
    AlreadyCommitted,
    /// Commit abandoned before barrier 1; the old state is in place.
    Cancelled,
    /// Commit failed after I/O started. Old or new state may be durable; the
    /// file is poisoned until it is reopened.
    Uncertain(CowError),
    /// An earlier commit failed: reopen (reconcile) before doing anything.
    ReconcileRequired,
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cow transaction error: {self:?}")
    }
}

impl std::error::Error for TxError {}

pub struct CowTx<'f, B: StorageBackend> {
    file: &'f mut CowFile<B>,
    /// Logical block count of the published mapping at `begin`.
    base_blocks: u64,
    dirty: BTreeMap<u64, Vec<u8>>,
    state: TxState,
}

impl<'f, B: StorageBackend> CowTx<'f, B> {
    /// Starts a transaction on the published state. No backend call.
    pub fn begin(file: &'f mut CowFile<B>) -> Result<Self, TxError> {
        if file.is_poisoned() {
            return Err(TxError::ReconcileRequired);
        }
        let base_blocks = file.block_count();
        Ok(Self {
            file,
            base_blocks,
            dirty: BTreeMap::new(),
            state: TxState::Open,
        })
    }

    pub fn state(&self) -> TxState {
        self.state
    }

    /// Number of dirty blocks held in memory.
    pub fn dirty_blocks(&self) -> usize {
        self.dirty.len()
    }

    /// The published mapping this transaction started from (old state until a
    /// commit succeeds).
    pub fn view(&self) -> &CowView<B> {
        self.file
    }

    fn require_open(&self) -> Result<(), TxError> {
        match self.state {
            TxState::Open => Ok(()),
            TxState::Uncertain => Err(TxError::ReconcileRequired),
            other => Err(TxError::NotOpen(other)),
        }
    }

    /// Writes `data` at byte `offset` of the logical disk (see the module
    /// docs). Either the whole range is applied or nothing is.
    pub fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), TxError> {
        self.require_open()?;
        if data.is_empty() {
            return Ok(());
        }
        let bs = BLOCK_SIZE as u64;
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(TxError::OutOfRange)?;
        let first = offset / bs;
        let last = (end - 1) / bs;
        if last >= MAX_BLOCKS {
            return Err(TxError::OutOfRange);
        }
        // Blocks that become dirty: the written range plus a zero gap between
        // the current end of the disk and the range.
        let current = self
            .base_blocks
            .max(self.dirty.keys().next_back().map_or(0, |k| k + 1));
        let lo = first.min(current);
        let span = last - lo + 1;
        let limit = MAX_TX_BLOCKS;
        if span > limit as u64 {
            return Err(TxError::TooLarge {
                limit,
                needed: usize::try_from(span).unwrap_or(usize::MAX),
            });
        }
        let new_blocks = (lo..=last).filter(|i| !self.dirty.contains_key(i)).count();
        let needed = self.dirty.len() + new_blocks;
        if needed > limit {
            return Err(TxError::TooLarge { limit, needed });
        }

        // Stage first (this is the only part that may read), apply after.
        let mut staged: Vec<(u64, Vec<u8>)> = Vec::with_capacity(span as usize);
        for index in lo..=last {
            let block_start = index * bs;
            let mut block = if let Some(d) = self.dirty.get(&index) {
                d.clone()
            } else if index < self.base_blocks
                && !(offset <= block_start && end >= block_start + bs)
            {
                // Partial cover of an existing block: read-modify-write.
                self.file.read_block(index).map_err(TxError::Rejected)?
            } else {
                vec![0u8; BLOCK_SIZE]
            };
            let from = offset.max(block_start);
            let to = end.min(block_start + bs);
            if from < to {
                let src = usize::try_from(from - offset)
                    .ok()
                    .zip(usize::try_from(to - offset).ok())
                    .and_then(|(a, b)| data.get(a..b));
                let dst = usize::try_from(from - block_start)
                    .ok()
                    .zip(usize::try_from(to - block_start).ok())
                    .and_then(|(a, b)| block.get_mut(a..b));
                match (src, dst) {
                    (Some(src), Some(dst)) => dst.copy_from_slice(src),
                    _ => return Err(TxError::OutOfRange),
                }
            }
            staged.push((index, block));
        }
        self.dirty.extend(staged);
        Ok(())
    }

    /// Publishes every dirty block as one generation. See [`TxError`].
    pub fn commit(&mut self) -> Result<DurableAck, TxError> {
        self.commit_cancellable(&mut |_| false)
    }

    /// [`commit`](Self::commit) with a cancel hook asked at every
    /// [`CommitStep`] before barrier 1.
    pub fn commit_cancellable(
        &mut self,
        cancel: &mut dyn FnMut(CommitStep) -> bool,
    ) -> Result<DurableAck, TxError> {
        self.require_open()?;
        if self.dirty.is_empty() {
            return Err(TxError::Rejected(CowError::BadWrite("empty transaction")));
        }
        let writes: Vec<(u64, &[u8])> =
            self.dirty.iter().map(|(i, d)| (*i, d.as_slice())).collect();
        match self.file.commit_cancellable(&writes, cancel) {
            Ok(ack) => {
                self.state = TxState::Committed;
                Ok(ack)
            }
            Err(CowError::Cancelled) => {
                self.state = TxState::Cancelled;
                Err(TxError::Cancelled)
            }
            Err(e) if self.file.is_poisoned() => {
                self.state = TxState::Uncertain;
                Err(TxError::Uncertain(e))
            }
            Err(e) => Err(TxError::Rejected(e)),
        }
    }

    /// Drops the dirty state. Before commit this never calls the backend; after
    /// a published commit it is refused.
    pub fn cancel(&mut self) -> Result<(), TxError> {
        match self.state {
            TxState::Open => {
                self.dirty.clear();
                self.state = TxState::Cancelled;
                Ok(())
            }
            TxState::Cancelled => Ok(()),
            TxState::Committed => Err(TxError::AlreadyCommitted),
            TxState::Uncertain => Err(TxError::ReconcileRequired),
        }
    }
}
