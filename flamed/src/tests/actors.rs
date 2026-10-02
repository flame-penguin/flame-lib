use std::sync::{Arc, Mutex};
use std::time::Duration;

use flamed_rpc::{ActorId, ActorResult, ClientError, FlamedApiClient, HttpClientBuilder};
use flamevm::{
    ActorID, CellDecode, CellEncode, CellEnvelope, Scalar, ScriptBuilder, String as VmString, Value,
};
use jsonrpsee::{core::client::ClientT, rpc_params};
use tempfile::TempDir;

use super::*;

fn counter_code() -> Vec<u8> {
    ScriptBuilder::new()
        .load()
        .push_int(0u64)
        .get()
        .push_int(1u64)
        .add()
        .put()
        .save()
        .to_bytecode()
}

pub(super) fn deploy(node: &mut Node, genesis: &GenesisFile) -> (ActorId, ContractID) {
    let input = contract_of(node, &genesis.contracts[0].id.0);
    let constructor = ScriptBuilder::new()
        .push_int(1_024u64)
        .addstorage()
        .verify()
        .merge()
        .verify()
        .push_int(1u64)
        .push_int(0u64)
        .push_int(0u64)
        .push_int(2u64)
        .dict()
        .push_str(counter_code())
        .setcode()
        .load()
        .drop_()
        .save()
        .to_bytecode();
    let actor = ActorID::Constructor(constructor);
    let unsigned = ScriptBuilder::new()
        .push_str(VmString::contract(input.clone()))
        .input()
        .signtx()
        .push_int(20 * FLAME)
        .split()
        .push_int(1u64)
        .push_str(input.predicate.to_point().to_bytes().to_vec())
        .push_int(1_000_000u64)
        .push_str(actor.to_envelope().unwrap().encode())
        .send()
        .push_point(input.predicate.to_point().to_bytes())
        .output()
        .build_tx(header(), limits())
        .unwrap();
    let change = created(unsigned.log())[0].id();
    let a = account(&SEED_A);
    let signed = sign(unsigned, &[a.spending_key_at(util::RECEIVING, 0).unwrap()]).unwrap();
    let tx = block_tx(signed, limits(), vec![live(node, &input.id())]);
    node.submit(&submitted(&tx)).unwrap();
    node.mint_block().unwrap();
    (ActorId(actor.to_hash()), change)
}

pub(super) fn increment(node: &mut Node, actor: ActorId, input_id: ContractID) {
    send_messages(node, actor, input_id, &[1_000_000]);
    node.mint_block().unwrap();
}

pub(super) fn send_messages(
    node: &mut Node,
    actor: ActorId,
    input_id: ContractID,
    gas_limits: &[u64],
) -> ContractID {
    let input = contract_of(node, &input_id);
    let mut script = ScriptBuilder::new()
        .push_str(VmString::contract(input.clone()))
        .input()
        .signtx();
    for gas in gas_limits {
        script = script
            .push_int(0u64)
            .push_str(input.predicate.to_point().to_bytes().to_vec())
            .push_int(*gas)
            .push_str(actor.0.to_vec())
            .send();
    }
    let unsigned = script
        .push_point(input.predicate.to_point().to_bytes())
        .output()
        .build_tx(header(), limits())
        .unwrap();
    let change = created(unsigned.log())[0].id();
    let a = account(&SEED_A);
    let signed = sign(unsigned, &[a.spending_key_at(util::RECEIVING, 0).unwrap()]).unwrap();
    let tx = block_tx(signed, limits(), vec![live(node, &input_id)]);
    node.submit(&submitted(&tx)).unwrap();
    change
}

fn counter_state(snapshot: &ActorResult) -> (u64, u64) {
    let bytes = &snapshot.state.as_ref().unwrap().0;
    let mut envelope = CellEnvelope::decode(bytes, 64 * 1024, &mut 1_000_000).unwrap();
    assert_eq!(envelope.root(), snapshot.state_hash);
    let root = envelope.cells().get(&envelope.root()).unwrap();
    let Value::Dict(mut dict) = Value::from_cell(&root, &mut envelope).unwrap() else {
        panic!("expected counter dictionary");
    };
    let Some(Value::Scalar(count)) = dict.get_resolved(&Scalar::ZERO, &mut envelope).unwrap()
    else {
        panic!("expected count");
    };
    let count = count.to_u64().unwrap();
    let Some(Value::ClearToken(reserve)) = dict.get_resolved(&Scalar::ONE, &mut envelope).unwrap()
    else {
        panic!("expected reserve");
    };
    assert_eq!(reserve.flv(), FLAME_FLAVOR);
    (count, reserve.qty().to_u64().unwrap())
}

#[test]
fn snapshots_do_not_mutate_actors_and_survive_replay_and_expiry() {
    let dir = TempDir::new().unwrap();
    let a = account(&SEED_A);
    let (_, cfg) = devnet(dir.path(), &a);
    let mut params = chainparams(&a);
    params.storage.lease_duration_blocks = Some(3);
    let path = cfg.genesis.as_ref().unwrap();
    crate::genesis::write(&params, path).unwrap();
    let genesis = GenesisFile::load(path).unwrap();
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let (id, change) = deploy(&mut node, &genesis);
    let snapshot = node.actor(&id.0).unwrap();
    assert_eq!(snapshot.height, 1);
    assert_eq!(snapshot.block.0, node.tip().hash.into_bytes());
    assert_eq!(snapshot.id, id);
    assert_eq!(snapshot.code.as_ref().unwrap().0, counter_code());
    assert_eq!(snapshot.code_hash, flamevm::code_root(&counter_code()));
    assert_eq!(snapshot.code_size, counter_code().len() as u64);
    assert!(snapshot.state_size > 0);
    assert!(snapshot.storage_used > 0);
    assert!(snapshot.storage_used <= snapshot.storage_capacity);
    assert_eq!(snapshot.storage_capacity, 1_024);
    let (count, reserve) = counter_state(&snapshot);
    assert_eq!(count, 0);
    assert!(reserve > 0 && reserve < 20 * FLAME);
    assert_eq!(node.actor(&id.0).unwrap(), snapshot);
    drop(node);

    let mut node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.actor(&id.0).unwrap(), snapshot);
    increment(&mut node, id, change);
    let updated = node.actor(&id.0).unwrap();
    assert_eq!(counter_state(&updated), (1, reserve));
    assert_ne!(updated.state_hash, snapshot.state_hash);
    assert_eq!(updated.code_hash, snapshot.code_hash);
    node.mint_block().unwrap();
    assert!(node.actor(&id.0).unwrap().state.is_some());
    node.mint_block().unwrap(); // Lease bought at 1 expires at 4.
    let frozen = node.actor(&id.0).unwrap();
    assert_eq!(frozen.height, 4);
    assert!(frozen.state.is_none());
    assert!(frozen.code.is_none());
    assert_eq!(frozen.state_hash, updated.state_hash);
    assert_eq!(frozen.code_hash, updated.code_hash);
    assert_eq!(frozen.code_size, updated.code_size);
    assert_eq!(frozen.state_size, updated.state_size);
    assert_eq!(frozen.storage_used, 0);
    assert_eq!(frozen.storage_capacity, 0);
    drop(node);
    let node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.actor(&id.0).unwrap(), frozen);
}

#[tokio::test]
async fn actor_over_http_and_lookup_errors() {
    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open(&genesis, &cfg).unwrap();
    let (id, _) = deploy(&mut node, &genesis);
    let expected = node.actor(&id.0).unwrap();
    let (addr, handle) = crate::rpc::serve(Arc::new(Mutex::new(node)), cfg.rpc_bind)
        .await
        .unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{addr}"))
        .unwrap();
    assert_eq!(client.actor(id).await.unwrap(), expected);
    assert_eq!(counter_state(&client.actor(id).await.unwrap()).0, 0);
    let ClientError::Call(error) = client.actor(ActorId([0xff; 32])).await.unwrap_err() else {
        panic!("expected JSON-RPC error");
    };
    assert_eq!(error.code(), flamed_rpc::codes::NOT_FOUND);
    let invalid: std::result::Result<ActorResult, _> =
        client.request("actor", rpc_params!["abcd"]).await;
    let ClientError::Call(error) = invalid.unwrap_err() else {
        panic!("expected invalid params");
    };
    assert_eq!(error.code(), flamed_rpc::ErrorCode::InvalidParams.code());
    drop(client);
    handle.stop().unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle.stopped())
        .await
        .unwrap();
}
