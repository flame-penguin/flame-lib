//! Transactions by id.

use std::collections::BTreeMap;

use flamechain::BlockHash;
use flamed_rpc::{
    ActorId, ActorStateChange, BlockSummary, BlockTxEnvelope, BlocksResult, Effect, ExecutionType,
    TransactionResult, TransactionSummary, TransactionsResult,
};
use flamevm::TxID;

/// Where a transaction landed, and what it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxRecord {
    /// The height of the block that carried it.
    pub height: u64,
    /// That block's hash.
    pub block: BlockHash,
    /// Its effect log, as a `CellEnvelope`.
    pub log: Vec<u8>,
    /// Public explorer summary; assigned a stable sequence on insertion.
    pub summary: TransactionSummary,
    /// Original signed bytes for external transactions.
    pub raw: Option<BlockTxEnvelope>,
    /// Bounded previews of the public effects.
    pub effects: Vec<Effect>,
    /// State boundaries captured before any later execution can overwrite them.
    pub actor_states: Vec<ActorStateChange>,
}

/// Confirmed transactions, by id.
#[derive(Debug, Default)]
pub struct TxIndex {
    // Keyed on the raw bytes: `TxID` is `Eq` but not `Ord`, the same reason
    // upstream's own mempool keeps a `BTreeSet<[u8; 32]>`.
    txs: BTreeMap<[u8; 32], TxRecord>,
    by_height: BTreeMap<u64, Vec<TxID>>,
    order: Vec<TxID>,
    by_actor: BTreeMap<ActorId, Vec<usize>>,
    blocks: BTreeMap<u64, BlockSummary>,
    counts: [u64; 3],
}

impl TxIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one confirmed transaction.
    pub fn insert(&mut self, txid: TxID, mut record: TxRecord) {
        record.summary.sequence = self.order.len() as u64;
        for actor in &record.summary.actors {
            self.by_actor
                .entry(*actor)
                .or_default()
                .push(self.order.len());
        }
        self.counts[match record.summary.kind {
            ExecutionType::External => 0,
            ExecutionType::Internal => 1,
            ExecutionType::InternalFailed => 2,
        }] += 1;
        self.order.push(txid);
        self.by_height.entry(record.height).or_default().push(txid);
        self.txs.insert(txid.0, record);
    }

    /// What the node knows about one transaction.
    pub fn get(&self, txid: &TxID) -> Option<&TxRecord> {
        self.txs.get(&txid.0)
    }

    /// Every transaction confirmed at one height, in block order.
    pub fn at_height(&self, height: u64) -> &[TxID] {
        self.by_height
            .get(&height)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// How many transactions have confirmed.
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    /// Whether any have.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Adds a header after the block has been accepted, including empty blocks.
    pub fn insert_block(&mut self, block: BlockSummary) {
        self.blocks.insert(block.height, block);
    }

    /// Header at an exact height.
    pub fn block(&self, height: u64) -> Option<&BlockSummary> {
        self.blocks.get(&height)
    }

    /// Bounded page, with an exclusive height cursor.
    pub fn blocks(&self, before: Option<u64>, limit: usize) -> BlocksResult {
        let mut page: Vec<_> = self
            .blocks
            .range(..before.unwrap_or(u64::MAX))
            .rev()
            .take(limit + 1)
            .map(|(_, b)| b.clone())
            .collect();
        let more = page.len() > limit;
        page.truncate(limit);
        let next = if more {
            page.last().map(|b| b.height)
        } else {
            None
        };
        BlocksResult { blocks: page, next }
    }

    /// Bounded execution page; actor lookup uses its own index.
    pub fn transactions(
        &self,
        before: Option<u64>,
        limit: usize,
        actor: Option<ActorId>,
    ) -> TransactionsResult {
        let end = before.unwrap_or(u64::MAX).min(self.order.len() as u64) as usize;
        let positions: Vec<usize> = if let Some(actor) = actor {
            let ids = self
                .by_actor
                .get(&actor)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let split = ids.partition_point(|i| *i < end);
            ids[..split].iter().rev().take(limit + 1).copied().collect()
        } else {
            (0..end).rev().take(limit + 1).collect()
        };
        let more = positions.len() > limit;
        let transactions: Vec<_> = positions
            .into_iter()
            .take(limit)
            .map(|i| self.txs[&self.order[i].0].summary.clone())
            .collect();
        let next = if more {
            transactions.last().map(|t| t.sequence)
        } else {
            None
        };
        TransactionsResult { transactions, next }
    }

    /// Confirmed transaction detail, including internal deliveries.
    pub fn transaction(&self, id: &TxID) -> Option<TransactionResult> {
        self.get(id).map(|r| TransactionResult {
            summary: r.summary.clone(),
            raw: r.raw.clone(),
            log: r.log.clone(),
            effects: r.effects.clone(),
            actor_states: r.actor_states.clone(),
        })
    }

    /// External, successful internal, and failed internal totals, respectively.
    pub fn counts(&self) -> [u64; 3] {
        self.counts
    }
}
