//! The methods `flamed` serves.
//!
//! The macro generates `FlamedApiServer`, which `flamed` implements, and
//! `FlamedApiClient`, blanket-implemented for every jsonrpsee client, so the
//! wire is written once and a wallet, an explorer or a monitor speaks it
//! without repeating a single method name.
//!
//! Nothing else belongs in this module: attributes on the trait are discarded
//! with it, so a lint allow would have to be module-level, and an item named
//! `core` or `jsonrpsee` here would shadow the paths the macro emits.

use crate::explorer::{
    ActorDetailResult, BlockResult, BlocksResult, ContractDetailResult, NetworkResult,
    TransactionResult, TransactionsResult,
};
use jsonrpsee::core::RpcResult;
use jsonrpsee::proc_macros::rpc;

use crate::types::{
    ActorId, ActorResult, BlockTxEnvelope, ContractId, ContractResult, PredicatePoint, ProofResult,
    ScanResult, TipResult, TxId, TxStatusResult,
};

/// The `flamed` JSON-RPC 2.0 API. Parameters are positional, in the order
/// written; method names carry no prefix.
#[rpc(server, client)]
pub trait FlamedApi {
    /// Network counters and a bounded mempool preview at the current tip.
    #[method(name = "network")]
    async fn network(&self) -> RpcResult<NetworkResult>;

    /// Blocks newest first, strictly below `before` when provided. Limit 1..100.
    #[method(name = "blocks")]
    async fn blocks(&self, before: Option<u64>, limit: u32) -> RpcResult<BlocksResult>;

    /// One block by height, including its internal executions.
    #[method(name = "block")]
    async fn block(&self, height: u64) -> RpcResult<BlockResult>;

    /// Confirmed executions newest first, optionally involving one actor.
    /// `before` is an exclusive execution sequence cursor. Limit 1..100.
    #[method(name = "transactions")]
    async fn transactions(
        &self,
        before: Option<u64>,
        limit: u32,
        actor: Option<ActorId>,
    ) -> RpcResult<TransactionsResult>;

    /// One confirmed external or internal transaction with its public effects.
    #[method(name = "tx")]
    async fn tx(&self, id: TxId) -> RpcResult<TransactionResult>;

    /// Current actor state and code, with bounded decoded views.
    #[method(name = "actor_detail")]
    async fn actor_detail(&self, id: ActorId) -> RpcResult<ActorDetailResult>;

    /// An archived contract, its spend status at one tip and bounded public payload.
    #[method(name = "contract_detail")]
    async fn contract_detail(&self, id: ContractId) -> RpcResult<ContractDetailResult>;

    /// The current tip: its hash, its height, and the contract accumulator root.
    #[method(name = "tip")]
    async fn tip(&self) -> RpcResult<TipResult>;

    /// One contract's membership proof, or what became of it.
    #[method(name = "proof")]
    async fn proof(&self, id: ContractId) -> RpcResult<ProofResult>;

    /// The same question for up to `MAX_PROOF_IDS` contracts at once.
    #[method(name = "proofs")]
    async fn proofs(&self, ids: Vec<ContractId>) -> RpcResult<Vec<(ContractId, ProofResult)>>;

    /// One contract as the node archived it. `NOT_FOUND` if it has none.
    #[method(name = "contract")]
    async fn contract(&self, id: ContractId) -> RpcResult<ContractResult>;

    /// An actor's current code, state and storage. `NOT_FOUND` if absent.
    /// Reads committed state without executing the actor's code.
    #[method(name = "actor")]
    async fn actor(&self, id: ActorId) -> RpcResult<ActorResult>;

    /// Offers a transaction to the mempool. `INVALID_BYTES` if it does not
    /// decode, `MEMPOOL_REJECTED` if the pool refuses it.
    #[method(name = "submit_tx")]
    async fn submit_tx(&self, block_tx: BlockTxEnvelope) -> RpcResult<TxId>;

    /// Whether a transaction is unknown, waiting, or confirmed.
    #[method(name = "tx_status")]
    async fn tx_status(&self, txid: TxId) -> RpcResult<TxStatusResult>;

    /// Every contract created at `since_height` or later under one of these
    /// predicates, each with the note that followed it in its log.
    /// `LIMIT_EXCEEDED` past `MAX_SCAN_PREDICATES`.
    #[method(name = "scan")]
    async fn scan(
        &self,
        predicates: Vec<PredicatePoint>,
        since_height: u64,
    ) -> RpcResult<ScanResult>;
}
