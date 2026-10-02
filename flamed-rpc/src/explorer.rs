//! Read-only explorer views. Monetary quantities are strings to preserve precision.
use crate::{
    ActorId, ActorResult, BlockId, BlockTxEnvelope, ContractId, ContractResult, ProofResult,
    TipResult, TxId,
};
use serde::{Deserialize, Serialize};

/// Maximum number of items returned by a paginated explorer request.
pub const MAX_PAGE_SIZE: u32 = 100;

/// Execution category, including a failed message delivery and its refund.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionType {
    /// A signed transaction submitted by a client.
    External,
    /// Successful asynchronous message execution.
    Internal,
    /// Failed message delivery; effects describe its refund.
    InternalFailed,
}

/// Summary of a confirmed execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionSummary {
    /// Stable zero-based execution index, used as an exclusive pagination cursor.
    pub sequence: u64,
    /// Transaction / execution identifier.
    pub id: TxId,
    /// Execution category.
    pub kind: ExecutionType,
    /// Containing block height.
    pub height: u64,
    /// Containing block identifier.
    pub block: BlockId,
    /// Execution which sent the received message, if internal.
    pub parent: Option<TxId>,
    /// Target of the received message, if internal.
    pub actor: Option<ActorId>,
    /// All actors mentioned by this execution's effects or incoming message.
    pub actors: Vec<ActorId>,
    /// Fee in sparks, as an exact decimal string.
    pub fee_sparks: String,
    /// Number of consumed outputs.
    pub inputs: u32,
    /// Number of created outputs, including refunds.
    pub outputs: u32,
    /// Number of outgoing asynchronous messages.
    pub messages: u32,
    /// VM error text on failed internal delivery; not a consensus field.
    pub error: Option<String>,
}

/// Public effect in execution order. `data` contains JSON display fields;
/// the complete canonical representation is in `TransactionResult.log`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    /// Effect opcode name, such as `send`, `actor_save`, or `output`.
    pub kind: String,
    /// Display fields; large byte strings are explicitly truncated previews.
    pub data: serde_json::Value,
}

/// Bounded public state at an execution boundary, rather than the current tip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorStateSnapshot {
    /// Canonical state root, including when the state body is unavailable.
    pub state_hash: String,
    /// Logical encoded state size.
    pub size_bytes: u64,
    /// Public display value, with the same bounds as `actor_detail`.
    pub decoded_state: Option<serde_json::Value>,
    /// Why the state could not be displayed; not a VM execution error.
    pub state_error: Option<String>,
}

/// State of a receiving or modified actor around one execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorStateChange {
    /// Actor whose state was observed.
    pub actor: ActorId,
    /// None when the actor did not exist before this execution.
    pub before: Option<ActorStateSnapshot>,
    /// None when the actor does not exist after this execution.
    pub after: Option<ActorStateSnapshot>,
}

/// One confirmed execution and its public effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionResult {
    /// Summary and location.
    pub summary: TransactionSummary,
    /// Signed block transaction bytes for external transactions only.
    pub raw: Option<BlockTxEnvelope>,
    /// Canonical public effect log as a base64 CellEnvelope.
    #[serde(with = "crate::codec::base64")]
    pub log: Vec<u8>,
    /// Public effects, in execution order.
    pub effects: Vec<Effect>,
    /// Snapshots immediately before and after this execution's effects.
    /// Empty for executions that neither receive nor modify an actor.
    #[serde(default)]
    pub actor_states: Vec<ActorStateChange>,
}

/// A block header summary. Block headers contain no timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockSummary {
    /// Block height; genesis is zero.
    pub height: u64,
    /// Block identifier.
    pub id: BlockId,
    /// Previous block identifier; absent for genesis.
    pub parent: Option<BlockId>,
    /// Consensus protocol version.
    pub version: u32,
    /// External transaction count.
    pub transactions: u32,
    /// Internal execution count, including failures.
    pub internal: u32,
    /// Failed internal execution count.
    pub failed: u32,
    /// Serialized block size in bytes; zero for synthetic genesis.
    pub size_bytes: u64,
    /// Committed effects root, as hex.
    pub effects_root: String,
    /// Committed actor root, as hex.
    pub actors_root: String,
    /// Committed contract accumulator root, as hex.
    pub contracts_root: String,
}

/// A block and its execution summaries in consensus order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockResult {
    /// Header summary.
    pub summary: BlockSummary,
    /// Both external and internal executions.
    pub executions: Vec<TransactionSummary>,
}

/// A page of blocks in descending height order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocksResult {
    /// Entries in this page.
    pub blocks: Vec<BlockSummary>,
    /// Pass as `before` to read older blocks; null when exhausted.
    pub next: Option<u64>,
}

/// A page of confirmed executions, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionsResult {
    /// Entries in this page.
    pub transactions: Vec<TransactionSummary>,
    /// Exclusive sequence cursor for the next page; null when exhausted.
    pub next: Option<u64>,
}

/// One pending external transaction; inclusion is not guaranteed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTransaction {
    /// Transaction id.
    pub id: TxId,
    /// Offered fee in sparks.
    pub fee_sparks: String,
}

/// Network counters at one consistent tip, with a bounded mempool preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkResult {
    /// Current chain tip.
    pub tip: TipResult,
    /// Confirmed external transactions.
    pub external: u64,
    /// Internal executions, including failures.
    pub internal: u64,
    /// Failed internal executions.
    pub failed: u64,
    /// Total pending transactions.
    pub mempool_total: u64,
    /// First 100 pending transactions in admission order.
    pub mempool: Vec<PendingTransaction>,
    /// Number of unspent outputs indexed by the node.
    pub unspent_outputs: u64,
}

/// One bytecode instruction as text; never executed by inspection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionView {
    /// Byte offset in the original code.
    pub offset: u64,
    /// Mnemonic and inline arguments, with bounded byte previews.
    pub text: String,
}

/// An actor snapshot with bounded human-readable views.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorDetailResult {
    /// Unmodified snapshot, including canonical bytes.
    pub snapshot: ActorResult,
    /// Decoded public state, if available and within display bounds.
    pub decoded_state: Option<serde_json::Value>,
    /// Why state could not be fully displayed.
    pub state_error: Option<String>,
    /// Disassembled code prefix, at most 10000 instructions.
    pub instructions: Vec<InstructionView>,
    /// Malformed code or a display limit, if encountered.
    pub code_error: Option<String>,
}

/// An archived output with its current spend status and bounded public payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractDetailResult {
    /// Canonical contract identifier.
    pub id: ContractId,
    /// Creation metadata and the original public contract envelope.
    pub snapshot: ContractResult,
    /// Identifier of the block that created this output, including genesis.
    pub created_block: BlockId,
    /// Identifier of the spending block, absent while unspent.
    pub spent_block: Option<BlockId>,
    /// Spend status; an unspent proof is valid against `tip`.
    pub status: ProofResult,
    /// Chain tip at which the spend status was read.
    pub tip: TipResult,
    /// Contract anchor, as hex.
    pub anchor: String,
    /// Size of the serialized contract envelope in bytes.
    pub size_bytes: u64,
    /// Decoded public payload; confidential tokens expose commitments only.
    pub decoded_payload: Option<serde_json::Value>,
    /// Why the payload could not be fully displayed.
    pub payload_error: Option<String>,
}
