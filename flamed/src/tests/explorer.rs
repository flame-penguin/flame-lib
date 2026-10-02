//! Explorer history must reflect actual executions and survive archive replay.
use std::sync::{Arc, Mutex};

use flamed_rpc::{
    ActorId, ClientError, ContractId, ExecutionType, FlamedApiClient, HttpClientBuilder,
    ProofResult, TxId,
};
use flamevm::{ScriptBuilder, String as VmString};
use tempfile::TempDir;

use super::*;

#[test]
fn execution_history_links_messages_paginates_and_replays() {
    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.blocks(None, 100).unwrap().blocks.len(), 1);
    assert!(node.block(0).unwrap().summary.parent.is_none());
    assert!(node
        .transactions(None, 1, None)
        .unwrap()
        .transactions
        .is_empty());

    let (actor, change) = actors::deploy(&mut node, &genesis);
    let deploy = node.block(1).unwrap();
    assert_eq!(deploy.summary.transactions, 1);
    assert_eq!(deploy.summary.internal, 1);
    assert_eq!(deploy.summary.failed, 0);
    assert_eq!(deploy.executions.len(), 2);
    let external = &deploy.executions[0];
    let internal = &deploy.executions[1];
    assert_eq!(external.kind, ExecutionType::External);
    assert_eq!(internal.kind, ExecutionType::Internal);
    assert_eq!(internal.parent, Some(external.id));
    assert_eq!(internal.actor, Some(actor));
    let detail = node.transaction(internal.id).unwrap();
    assert!(detail.raw.is_none());
    assert_eq!(detail.actor_states.len(), 1);
    assert_eq!(detail.actor_states[0].actor, actor);
    assert!(detail.actor_states[0].before.is_none());
    let deployed_state = detail.actor_states[0].after.as_ref().unwrap();
    assert_eq!(
        deployed_state.decoded_state.as_ref().unwrap()["entries"]["0"]["value"],
        "0"
    );
    for kind in [
        "receive",
        "actor_deploy",
        "actor_save",
        "set_code",
        "storage_purchase",
    ] {
        assert!(detail.effects.iter().any(|e| e.kind == kind), "{kind}");
    }
    let signed = node.transaction(external.id).unwrap().raw.unwrap();
    assert_eq!(
        BlockTx::from_bytes_bounded(
            &signed.0,
            genesis.chain.version,
            genesis.chain.limits.resolve()
        )
        .unwrap()
        .tx
        .txid
        .0,
        external.id.0
    );

    // An update log may reference previously resident state cells. Index it
    // directly while borrowed, without requiring a standalone decoded log.
    actors::increment(&mut node, actor, change);
    let updated = node
        .transaction(node.block(2).unwrap().executions[1].id)
        .unwrap();
    let state_change = &updated.actor_states[0];
    assert_eq!(state_change.before.as_ref().unwrap(), deployed_state);
    assert_eq!(
        state_change
            .after
            .as_ref()
            .unwrap()
            .decoded_state
            .as_ref()
            .unwrap()["entries"]["0"]["value"],
        "1"
    );
    assert_eq!(
        state_change.after.as_ref().unwrap().state_hash,
        hex::encode(node.actor(&actor.0).unwrap().state_hash)
    );
    node.mint_block().unwrap(); // Empty blocks also belong in the explorer.
    let network = node.network();
    assert_eq!(
        (network.external, network.internal, network.failed),
        (2, 2, 0)
    );
    assert_eq!(network.tip.height, 3);
    let all = node.transactions(None, 100, None).unwrap();
    assert_eq!(all.transactions.len(), 4);
    let first = node.transactions(None, 2, Some(actor)).unwrap();
    let second = node.transactions(first.next, 2, Some(actor)).unwrap();
    assert!(second.next.is_none());
    assert_eq!(
        [first.transactions, second.transactions].concat(),
        all.transactions
    );
    assert!(node
        .transactions(Some(0), 100, None)
        .unwrap()
        .transactions
        .is_empty());
    assert!(node
        .transactions(None, 100, Some(ActorId([0; 32])))
        .unwrap()
        .transactions
        .is_empty());
    let first = node.blocks(None, 2).unwrap();
    let second = node.blocks(first.next, 2).unwrap();
    assert_eq!(
        first.blocks.iter().map(|b| b.height).collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert_eq!(
        second.blocks.iter().map(|b| b.height).collect::<Vec<_>>(),
        vec![1, 0]
    );
    assert!(second.next.is_none());
    assert!(node.blocks(Some(0), 100).unwrap().blocks.is_empty());
    let actor_detail = crate::inspect::actor_detail(node.actor(&actor.0).unwrap());
    assert!(actor_detail.state_error.is_none());
    assert!(actor_detail.code_error.is_none());
    assert_eq!(
        actor_detail.decoded_state.as_ref().unwrap()["entries"]["0"]["value"],
        "1"
    );
    assert_eq!(actor_detail.instructions.first().unwrap().text, "load");
    assert_eq!(actor_detail.instructions.last().unwrap().text, "save");
    let details: Vec<_> = all
        .transactions
        .iter()
        .map(|t| node.transaction(t.id).unwrap())
        .collect();
    let blocks = node.blocks(None, 100).unwrap();
    drop(node);
    let node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.network(), network);
    assert_eq!(node.blocks(None, 100).unwrap(), blocks);
    assert_eq!(node.transactions(None, 100, Some(actor)).unwrap(), all);
    for detail in details {
        assert_eq!(node.transaction(detail.summary.id).unwrap(), detail);
    }
    assert_eq!(
        crate::inspect::actor_detail(node.actor(&actor.0).unwrap()),
        actor_detail
    );
}

#[test]
fn failed_delivery_is_indexed_with_a_spendable_refund() {
    let dir = TempDir::new().unwrap();
    let a = account(&SEED_A);
    let (genesis, cfg) = devnet(dir.path(), &a);
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let input = contract_of(&node, &genesis.contracts[0].id.0);
    let missing = ActorId([0x77; 32]);
    let unsigned = ScriptBuilder::new()
        .push_str(VmString::contract(input.clone()))
        .input()
        .signtx()
        .push_int(20 * FLAME)
        .split()
        .push_int(1u64)
        .push_str(input.predicate.to_point().to_bytes().to_vec())
        .push_int(1_000_000u64)
        .push_str(missing.0.to_vec())
        .send()
        .push_point(input.predicate.to_point().to_bytes())
        .output()
        .build_tx(header(), limits())
        .unwrap();
    let signed = sign(unsigned, &[a.spending_key_at(util::RECEIVING, 0).unwrap()]).unwrap();
    let tx = block_tx(signed, limits(), vec![live(&node, &input.id())]);
    let txid = node.submit(&submitted(&tx)).unwrap();
    assert_eq!(node.network().mempool_total, 1);
    assert_eq!(node.network().mempool[0].id.0, txid.0);
    node.mint_block().unwrap();
    assert_eq!(node.network().mempool_total, 0);
    assert_eq!(node.network().failed, 1);
    let block = node.block(1).unwrap();
    let bounced = node.transaction(block.executions[1].id).unwrap();
    assert_eq!(bounced.summary.kind, ExecutionType::InternalFailed);
    assert_eq!(bounced.summary.parent, Some(TxId(txid.0)));
    assert_eq!(bounced.summary.actor, Some(missing));
    assert_eq!(bounced.actor_states.len(), 1);
    assert!(bounced.actor_states[0].before.is_none());
    assert!(bounced.actor_states[0].after.is_none());
    assert!(bounced
        .summary
        .error
        .as_ref()
        .is_some_and(|e| !e.is_empty()));
    let refund = bounced.effects.iter().find(|e| e.kind == "output").unwrap();
    let id: ContractID = hex::decode(refund.data["contract"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    assert!(node.verify_proof(&id, &live(&node, &id)));
    let contract = contract_of(&node, &id);
    let flamevm::Value::Dict(mut payload) = contract.payload().clone() else {
        panic!("refund payload")
    };
    let flamevm::Value::ClearToken(value) = payload.remove(&flamevm::Scalar::ZERO).unwrap() else {
        panic!("refund token")
    };
    assert_eq!(value.qty().to_u64().unwrap(), 20 * FLAME);
    drop(node);
    let node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.transaction(bounced.summary.id).unwrap(), bounced);
    assert!(node.verify_proof(&id, &live(&node, &id)));
}

#[test]
fn state_snapshots_are_per_execution_and_failed_calls_leave_them_unchanged() {
    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let (actor, change) = actors::deploy(&mut node, &genesis);
    let change = actors::send_messages(&mut node, actor, change, &[1_000_000, 1_000_000]);
    node.mint_block().unwrap();
    let block = node.block(2).unwrap();
    assert_eq!(block.executions.len(), 3);
    assert!(node
        .transaction(block.executions[0].id)
        .unwrap()
        .actor_states
        .is_empty());
    let first = node.transaction(block.executions[1].id).unwrap();
    let second = node.transaction(block.executions[2].id).unwrap();
    let count = |snapshot: &flamed_rpc::ActorStateSnapshot| {
        snapshot.decoded_state.as_ref().unwrap()["entries"]["0"]["value"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    let reserve = |snapshot: &flamed_rpc::ActorStateSnapshot| {
        snapshot.decoded_state.as_ref().unwrap()["entries"]["1"].clone()
    };
    let first = &first.actor_states[0];
    let second = &second.actor_states[0];
    assert_eq!(count(first.before.as_ref().unwrap()), 0);
    assert_eq!(count(first.after.as_ref().unwrap()), 1);
    assert_eq!(second.before, first.after);
    assert_eq!(count(second.after.as_ref().unwrap()), 2);
    assert_eq!(
        reserve(first.before.as_ref().unwrap()),
        reserve(second.after.as_ref().unwrap())
    );
    actors::send_messages(&mut node, actor, change, &[1]);
    node.mint_block().unwrap();
    let failed = node
        .transaction(node.block(3).unwrap().executions[1].id)
        .unwrap();
    assert_eq!(failed.summary.kind, ExecutionType::InternalFailed);
    assert_eq!(failed.actor_states[0].before, second.after);
    assert_eq!(failed.actor_states[0].after, second.after);
    drop(node);
    let node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.transaction(failed.summary.id).unwrap(), failed);
    assert_eq!(
        node.transaction(block.executions[1].id)
            .unwrap()
            .actor_states[0],
        *first
    );
    assert_eq!(
        node.transaction(block.executions[2].id)
            .unwrap()
            .actor_states[0],
        *second
    );
}

#[tokio::test]
async fn explorer_methods_and_bounds_over_http() {
    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let genesis_id = genesis.contracts[0].id;
    let (actor, change) = actors::deploy(&mut node, &genesis);
    let a = account(&SEED_A);
    let input = InputSpec::clear(
        contract_of(&node, &change),
        live(&node, &change),
        a.spending_key_at(util::RECEIVING, 0).unwrap(),
    )
    .unwrap();
    let output = output(&a, util::CHANGE, 0, 980 * FLAME - FEE);
    let (transfer, outputs) = signed_transfer(vec![input], &[output], FEE, &mut rng(70));
    let change_id = ContractId(outputs[0].id());
    node.submit(&submitted(&transfer)).unwrap();
    node.mint_block().unwrap();
    let network = node.network();
    let block = node.block(1).unwrap();
    let expected = node.transaction(block.executions[1].id).unwrap();
    let (addr, handle) = crate::rpc::serve(Arc::new(Mutex::new(node)), cfg.rpc_bind)
        .await
        .unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{addr}"))
        .unwrap();
    assert_eq!(client.network().await.unwrap(), network);
    assert_eq!(
        client.blocks(Some(2), 1).await.unwrap().blocks[0],
        block.summary
    );
    assert_eq!(client.block(1).await.unwrap(), block);
    assert_eq!(client.tx(expected.summary.id).await.unwrap(), expected);
    let history = client.transactions(None, 100, Some(actor)).await.unwrap();
    assert_eq!(history.transactions.len(), 2);
    assert!(client
        .actor_detail(actor)
        .await
        .unwrap()
        .state_error
        .is_none());
    let spent = client.contract_detail(genesis_id).await.unwrap();
    assert_eq!(spent.id, genesis_id);
    assert_eq!(spent.snapshot.height, 0);
    assert_eq!(spent.tip, network.tip);
    assert!(matches!(
        spent.status,
        ProofResult::Spent { height: 1, txid } if txid == block.executions[0].id
    ));
    assert_eq!(spent.snapshot, client.contract(genesis_id).await.unwrap());
    let canonical = contract_from_bytes(&spent.snapshot.bytes.0).unwrap();
    assert_eq!(spent.anchor, hex::encode(canonical.anchor.0));
    assert_eq!(spent.size_bytes, spent.snapshot.bytes.0.len() as u64);
    assert!(spent.payload_error.is_none());
    assert_eq!(
        spent.decoded_payload.as_ref().unwrap()["type"],
        "clear_token"
    );
    assert_eq!(
        spent.decoded_payload.as_ref().unwrap()["quantity"],
        GENESIS_SPARKS.to_string()
    );
    let unspent = client.contract_detail(change_id).await.unwrap();
    assert_eq!(unspent.tip, network.tip);
    assert_eq!(unspent.snapshot.height, 2);
    assert!(matches!(unspent.status, ProofResult::Unspent { .. }));
    assert!(unspent.payload_error.is_none());
    let payload = unspent.decoded_payload.as_ref().unwrap();
    assert_eq!(payload["type"], "token");
    assert!(payload.get("quantity").is_none());
    assert!(payload.get("flavor").is_none());
    assert_eq!(payload["quantity_commitment"].as_str().unwrap().len(), 64);
    assert_eq!(payload["flavor_commitment"].as_str().unwrap().len(), 64);
    for error in [
        client.blocks(None, 0).await.unwrap_err(),
        client.transactions(None, 101, None).await.unwrap_err(),
    ] {
        let ClientError::Call(error) = error else {
            panic!("RPC error")
        };
        assert_eq!(error.code(), flamed_rpc::codes::LIMIT_EXCEEDED);
    }
    for error in [
        client.block(9).await.unwrap_err(),
        client.tx(TxId([0; 32])).await.unwrap_err(),
        client
            .contract_detail(ContractId([0; 32]))
            .await
            .unwrap_err(),
    ] {
        let ClientError::Call(error) = error else {
            panic!("RPC error")
        };
        assert_eq!(error.code(), flamed_rpc::codes::NOT_FOUND);
    }
    drop(client);
    handle.stop().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), handle.stopped())
        .await
        .unwrap();
}
