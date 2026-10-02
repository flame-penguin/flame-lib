//! Bounded public display views. Binary envelopes remain the source of truth.
use flamed_rpc::{
    ActorDetailResult, ActorResult, ActorStateSnapshot, BlockId, ContractDetailResult, ContractId,
    ContractResult, Effect, InstructionView, ProofResult, TipResult,
};
use flamevm::{
    CellDecode, CellEnvelope, CellError, CellResolver, Contract, Instruction, Scalar, TxEntry,
    TxLog, Value,
};
use serde_json::{json, Map, Value as Json};

fn scalar(n: Scalar) -> String {
    n.centered_abs()
        .to_u128()
        .map(|v| {
            if n.is_centered_negative() {
                format!("-{v}")
            } else {
                v.to_string()
            }
        })
        .unwrap_or_else(|| format!("0x{}", hex::encode(n.to_bytes())))
}

fn bytes_preview(bytes: &[u8]) -> Json {
    json!({"hex": hex::encode(&bytes[..bytes.len().min(256)]), "bytes": bytes.len(), "truncated": bytes.len() > 256})
}

fn value_json(
    value: Value,
    cells: &mut dyn CellResolver,
    depth: usize,
    left: &mut usize,
) -> Result<Json, CellError> {
    if depth > 32 || *left == 0 {
        return Err(CellError::LimitExceeded);
    }
    *left -= 1;
    Ok(match value {
        Value::Scalar(n) => json!({"type": "scalar", "value": scalar(n)}),
        Value::String(s) => json!({"type": "bytes", "value": bytes_preview(&s.to_bytes())}),
        Value::Point(p) => json!({"type": "point", "hex": hex::encode(p.to_bytes())}),
        Value::ClearToken(t) => {
            json!({"type": "clear_token", "quantity": scalar(t.qty()), "flavor": scalar(t.flv())})
        }
        Value::Token(t) => {
            json!({"type": "token", "quantity_commitment": hex::encode(t.qty().to_point().to_bytes()), "flavor_commitment": hex::encode(t.flv().to_point().to_bytes())})
        }
        Value::Dict(mut d) => {
            let mut entries = Map::new();
            let mut key = d.first_key_resolved(cells)?;
            while let Some(k) = key {
                let value = d
                    .get_resolved(&k, cells)?
                    .ok_or(CellError::InvalidFormat)?
                    .clone();
                entries.insert(scalar(k), value_json(value, cells, depth + 1, left)?);
                key = d.next_key_after_resolved(&k, cells)?;
            }
            json!({"type": "dict", "entries": entries})
        }
        _ => return Err(CellError::InvalidFormat),
    })
}

/// Makes bounded state and bytecode views without running the actor.
pub fn actor_detail(snapshot: ActorResult) -> ActorDetailResult {
    let decoded = snapshot.state.as_ref().map(|bytes| {
        let mut budget = (bytes.0.len() as u64).saturating_mul(4);
        let envelope = CellEnvelope::decode(&bytes.0, 64 * 1024 * 1024, &mut budget)?;
        decode_state(envelope)
    });
    let (decoded_state, state_error) = match decoded {
        Some(Ok(value)) => (Some(value), None),
        Some(Err(error)) => (None, Some(error.to_string())),
        None => (None, Some("State body is unavailable".into())),
    };
    let mut instructions = Vec::new();
    let mut code_error = None;
    if let Some(code) = &snapshot.code {
        let mut remaining = code.0.as_slice();
        while !remaining.is_empty() {
            let offset = (code.0.len() - remaining.len()) as u64;
            if instructions.len() == 10_000 {
                code_error = Some("Showing the first 10000 instructions".into());
                break;
            }
            match Instruction::parse(&mut remaining) {
                Ok(op) => instructions.push(InstructionView {
                    offset,
                    text: instruction_text(op),
                }),
                Err(error) => {
                    code_error = Some(format!("Byte {offset}: {error}"));
                    break;
                }
            }
        }
    } else {
        code_error = Some("Code body is unavailable".into());
    }
    ActorDetailResult {
        snapshot,
        decoded_state,
        state_error,
        instructions,
        code_error,
    }
}

fn decode_state(mut envelope: CellEnvelope) -> Result<Json, CellError> {
    let cell = envelope
        .cells()
        .get(&envelope.root())
        .ok_or(CellError::MissingCell(envelope.root()))?;
    let value = Value::from_cell(&cell, &mut envelope)?;
    value_json(value, &mut envelope, 0, &mut 2_000)
}

/// Decodes an archived contract without executing it or recovering private openings.
pub fn contract_detail(
    id: ContractId,
    snapshot: ContractResult,
    status: ProofResult,
    tip: TipResult,
    created_block: BlockId,
    spent_block: Option<BlockId>,
) -> Result<ContractDetailResult, CellError> {
    let size_bytes = snapshot.bytes.0.len() as u64;
    let mut budget = size_bytes.saturating_mul(4);
    let mut envelope = CellEnvelope::decode(&snapshot.bytes.0, 64 * 1024 * 1024, &mut budget)?;
    if envelope.root() != id.0 {
        return Err(CellError::InvalidFormat);
    }
    let root = envelope
        .cells()
        .get(&envelope.root())
        .ok_or(CellError::MissingCell(envelope.root()))?;
    let contract = Contract::from_cell(&root, &mut envelope)?;
    let anchor = hex::encode(contract.anchor.0);
    let (decoded_payload, payload_error) =
        match value_json(contract.into_payload(), &mut envelope, 0, &mut 2_000) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error.to_string())),
        };
    Ok(ContractDetailResult {
        id,
        snapshot,
        created_block,
        spent_block,
        status,
        tip,
        anchor,
        size_bytes,
        decoded_payload,
        payload_error,
    })
}

/// Captures public state for history without checking out or running the actor.
pub fn state_snapshot(info: flamechain::ActorInfo) -> ActorStateSnapshot {
    let (decoded_state, state_error) = match info.state.map(decode_state) {
        Some(Ok(value)) => (Some(value), None),
        Some(Err(error)) => (None, Some(error.to_string())),
        None => (None, Some("State body is unavailable".into())),
    };
    ActorStateSnapshot {
        state_hash: hex::encode(info.state_root),
        size_bytes: info.state_size,
        decoded_state,
        state_error,
    }
}

fn instruction_text(op: Instruction) -> String {
    match op {
        Instruction::PushInt(n) => format!("push {}", scalar(n)),
        Instruction::PushStr(s) => {
            let bytes = s.to_bytes();
            format!(
                "pushstr 0x{}{}",
                hex::encode(&bytes[..bytes.len().min(256)]),
                if bytes.len() > 256 {
                    "… (preview)"
                } else {
                    ""
                }
            )
        }
        Instruction::PushPoint(p) => format!("pushpoint 0x{}", hex::encode(p.to_bytes())),
        Instruction::DupK(k) => format!("dup:{k}"),
        Instruction::RollK(k) => format!("roll:{k}"),
        Instruction::Label(n) => format!("label:{n}"),
        Instruction::Jump(n) => format!("jump:{n}"),
        Instruction::JumpIf(n) => format!("jumpif:{n}"),
        Instruction::Ext(n) => format!("ext 0x{n:02x}"),
        Instruction::Alloc(_) => "alloc".into(),
        plain => format!("{plain:?}").to_ascii_lowercase(),
    }
}

/// Renders public effect metadata; bearer values never reveal private openings.
pub fn effects(log: &TxLog) -> Vec<Effect> {
    log.iter().map(|entry| {
        let (kind, data) = match entry {
            TxEntry::Header(h) => ("header", json!({"version": h.version, "locktime": h.locktime.to_string()})),
            TxEntry::CellWitness(id) => ("cell_witness", json!({"hash": hex::encode(id)})),
            TxEntry::Data(b) => ("data", bytes_preview(b)),
            TxEntry::Input(id) => ("input", json!({"contract": hex::encode(id)})),
            TxEntry::Receive(id) => ("receive", json!({"message": hex::encode(id)})),
            TxEntry::Output(c) => ("output", json!({"contract": hex::encode(c.id()), "predicate": hex::encode(c.predicate.to_point().to_bytes()), "confidential": matches!(c.payload(), Value::Token(_))})),
            TxEntry::ActorDeploy {actor, code} => ("actor_deploy", json!({"actor": hex::encode(actor.to_hash()), "code_bytes": code.len()})),
            TxEntry::ActorSave {actor, state} => ("actor_save", json!({"actor": hex::encode(actor.to_hash()), "state_hash": hex::encode(flamevm::state_root(state))})),
            TxEntry::SetCode {actor, code} => ("set_code", json!({"actor": hex::encode(actor.to_hash()), "code_hash": hex::encode(flamevm::code_root(code)), "code_bytes": code.len()})),
            TxEntry::ActorDestroy {actor} => ("actor_destroy", json!({"actor": hex::encode(actor.to_hash())})),
            TxEntry::Send(m) => ("send", json!({"message": hex::encode(m.id().0), "actor": hex::encode(m.target.to_hash()), "caller": m.caller.as_ref().map(|a| hex::encode(a.to_hash())), "gas_limit": m.gas.to_string(), "arguments": m.payload().len(), "deploy": matches!(m.target, flamevm::ActorID::Constructor(_))})),
            TxEntry::StoragePurchase {actor, bytes, expiry_height, fee_sparks} => ("storage_purchase", json!({"actor": hex::encode(actor.to_hash()), "bytes": bytes, "expiry_height": expiry_height, "fee_sparks": scalar(*fee_sparks)})),
            TxEntry::Fee(q) => ("fee", json!({"sparks": q.to_string()})),
            TxEntry::IssuePub(q, f) => ("issue_public", json!({"quantity": scalar(*q), "flavor": scalar(*f)})),
            TxEntry::IssuePriv(q, f) => ("issue_private", json!({"quantity_commitment": hex::encode(q.to_bytes()), "flavor_commitment": hex::encode(f.to_bytes())})),
            TxEntry::Retire(q, f) => ("retire", json!({"quantity_commitment": hex::encode(q.to_bytes()), "flavor_commitment": hex::encode(f.to_bytes())})),
        };
        Effect { kind: kind.into(), data }
    }).collect()
}
