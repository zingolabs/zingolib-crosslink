//! An end-to-end bond cycle against a local crosslink node: a new seed wallet takes faucet
//! funds, bonds to the node's finalizer, unbonds and withdraws, through the same backend and
//! reducer the terminal uses. The node mines only when asked, through its `generate` RPC.
//!
//! Needs a running node with the stable testnet's parameters and `disable_pow`, BFT started and
//! one finalizer on the roster: the `localnet.sh up` setup. Ignored by default; it mines some
//! 20,000 blocks and takes the better part of an hour.
//!
//! ```text
//! DELEGATOR_LOCAL_GRPC=http://127.0.0.1:18234 DELEGATOR_LOCAL_RPC=http://127.0.0.1:8232 \
//!   cargo test -p delegator-tui --release local_bond_cycle -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use zingolib::wallet::SeedDerivation;

use super::{BackendConfig, spawn};
use crate::app::{
    Action, BondStatus, Command, KeyMaterial, Network, OpenSpec, Overlay, Phase, StakeForm,
    StakeOp, StakeStage, StakingDay, State, reduce,
};

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Mines until the node's tip is `n` blocks higher, through its JSON-RPC. A `generate` call can
/// fail with its block made anyway, or with none made: another miner on the node
/// (localnet.sh's) built the same block at the same moment, a duplicate, or the node's gossip
/// queue, with no peer to drain it, was full. So the tip, not the answer, says how far it got;
/// only a run of failures with no progress stops it.
fn generate(n: u32) {
    let rpc = env("DELEGATOR_LOCAL_RPC", "http://127.0.0.1:8232");
    let target = node_tip() + n;
    let mut stuck = 0;
    while node_tip() < target {
        let before = node_tip();
        let batch = (target - before).min(50);
        let out = std::process::Command::new("curl")
            .args(["-s", "-m", "600", "-X", "POST", &rpc])
            .args(["-H", "content-type: application/json", "-d"])
            .arg(format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"generate","params":[{batch}]}}"#
            ))
            .output()
            .expect("curl");
        let answer = String::from_utf8_lossy(&out.stdout);
        if node_tip() > before {
            stuck = 0;
        } else {
            stuck += 1;
            assert!(stuck < 20, "generate makes no progress: {answer}");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// The node's tip, through its JSON-RPC.
fn node_tip() -> u32 {
    let rpc = env("DELEGATOR_LOCAL_RPC", "http://127.0.0.1:8232");
    let out = std::process::Command::new("curl")
        .args(["-s", "-m", "60", "-X", "POST", &rpc])
        .args(["-H", "content-type: application/json", "-d"])
        .arg(r#"{"jsonrpc":"2.0","id":1,"method":"getblockchaininfo","params":[]}"#)
        .output()
        .expect("curl");
    let text = String::from_utf8_lossy(&out.stdout);
    let at = text.find("\"blocks\":").expect("getblockchaininfo") + 9;
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// Folds backend actions into the state until `done` holds, forwarding what the reducer asks
/// for. Mines a block every few seconds while it waits, as the localnet miner would.
async fn drive(
    what: &str,
    state: &mut State,
    actions: &mut mpsc::UnboundedReceiver<Action>,
    commands: &mpsc::UnboundedSender<Command>,
    limit: Duration,
    done: impl Fn(&State) -> bool,
) {
    let deadline = Instant::now() + limit;
    let mut next_block = Instant::now();
    while !done(state) {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        if Instant::now() >= next_block {
            generate(1);
            next_block = Instant::now() + Duration::from_secs(4);
        }
        let Ok(Some(action)) = tokio::time::timeout(Duration::from_secs(1), actions.recv()).await
        else {
            continue;
        };
        if let Action::Notice(n) = &action {
            eprintln!("  notice: {n}");
        }
        for command in reduce(state, action) {
            if !matches!(command, Command::Copy(_) | Command::ForceQuit) {
                commands.send(command).unwrap();
            }
        }
    }
    eprintln!(
        "{:>8.1?} {what} · wallet at {:?}, node at {}",
        state.now.elapsed(),
        state.sync.tip,
        node_tip()
    );
}

/// The wallet has scanned up to the node's tip as it stood when the wait began.
fn synced_to(height: u32) -> impl Fn(&State) -> bool {
    move |s: &State| s.sync.sessions_done > 0 && s.sync.tip.is_some_and(|t| t >= height)
}

/// Runs the staking form for `op` from its first stage to its txid.
async fn act(
    op: StakeOp,
    state: &mut State,
    actions: &mut mpsc::UnboundedReceiver<Action>,
    commands: &mpsc::UnboundedSender<Command>,
) {
    let verb = op.verb();
    let command = match &op {
        StakeOp::Stake { finalizer } => Command::ProposeStake {
            finalizer: finalizer.clone(),
            amount: 10_000_000,
        },
        StakeOp::Unbond { bond } => Command::ProposeUnbond {
            bond: bond.key.clone(),
        },
        StakeOp::Redelegate { bond, to } => Command::ProposeRedelegate {
            bond: bond.key.clone(),
            to: to.clone(),
        },
        StakeOp::Withdraw { bond } => Command::Withdraw {
            bond: bond.key.clone(),
        },
    };
    let mut form = StakeForm::new(op);
    form.power = 7;
    form.stage = if matches!(command, Command::Withdraw { .. }) {
        StakeStage::Sending
    } else {
        StakeStage::Proposing
    };
    state.overlay = Overlay::Stake(form);
    commands.send(command).unwrap();
    let stage = |s: &State| match &s.overlay {
        Overlay::Stake(f) => f.stage.clone(),
        _ => StakeStage::Edit,
    };
    if stage(state) == StakeStage::Proposing {
        drive(
            &format!("{verb} proposed"),
            state,
            actions,
            commands,
            Duration::from_secs(300),
            |s| !matches!(stage(s), StakeStage::Proposing),
        )
        .await;
        match stage(state) {
            StakeStage::Confirm { fee } => eprintln!("  {verb} fee {fee:?}"),
            other => {
                let reason = match &state.overlay {
                    Overlay::Stake(f) => f.error.clone(),
                    _ => None,
                };
                panic!("{verb} refused: {other:?} {reason:?}")
            }
        }
        if let Overlay::Stake(f) = &mut state.overlay {
            f.stage = StakeStage::Sending;
        }
        commands.send(Command::ConfirmStake).unwrap();
    }
    drive(
        &format!("{verb} sent"),
        state,
        actions,
        commands,
        Duration::from_secs(600),
        |s| !matches!(stage(s), StakeStage::Sending),
    )
    .await;
    match stage(state) {
        StakeStage::Sent { txids } => eprintln!("  {verb} txid {}", txids.join(" ")),
        other => panic!("{verb} failed: {other:?}"),
    }
    state.overlay = Overlay::None;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local crosslink node: see localnet.sh"]
async fn local_bond_cycle() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let grpc = env("DELEGATOR_LOCAL_GRPC", "http://127.0.0.1:18234");
    let dir = tempfile::tempdir().unwrap();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (act_tx, mut act_rx) = mpsc::unbounded_channel();
    let _task = spawn(
        BackendConfig {
            data_dir: Some(dir.path().to_path_buf()),
            server: Some(grpc.clone()),
            seed_derivation: SeedDerivation::Zip32Standard,
        },
        cmd_rx,
        act_tx,
    );
    let mut s = State::new(Phase::Loading("opening".into()), Instant::now());

    let phrase =
        bip0039::Mnemonic::<bip0039::English>::generate(bip0039::Count::Words24).into_phrase();
    cmd_tx
        .send(Command::Open(OpenSpec::Create {
            network: Network::Stable,
            name: "e2e".into(),
            material: KeyMaterial::Seed(phrase),
            birthday: 1,
            server: Some(grpc),
        }))
        .unwrap();
    let tip = node_tip();
    drive(
        "opened and synced",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(600),
        synced_to(tip),
    )
    .await;
    drive(
        "roster",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(60),
        |s| !s.roster.is_empty(),
    )
    .await;
    let finalizer = s.roster[0].finalizer.clone();
    eprintln!("  finalizer {}…", &finalizer[..24]);

    let address = s
        .faucet_address()
        .expect("an orchard address")
        .address
        .clone();
    cmd_tx.send(Command::Faucet { address }).unwrap();
    drive(
        "faucet paid",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(600),
        |s| s.balance.total() > s.balance.pending() && s.balance.total() >= 20_000_000,
    )
    .await;

    let next = node_tip() + 1;
    let day = StakingDay::at_height(next);
    if !day.open {
        eprintln!("  no staking day at {next}: mining to {}", day.at);
        generate(day.at - next);
        drive(
            "synced to the staking day",
            &mut s,
            &mut act_rx,
            &cmd_tx,
            Duration::from_secs(600),
            synced_to(day.at - 1),
        )
        .await;
    }
    act(
        StakeOp::Stake {
            finalizer: finalizer.clone(),
        },
        &mut s,
        &mut act_rx,
        &cmd_tx,
    )
    .await;
    drive(
        "bond listed",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(300),
        |s| s.bonds.len() == 1 && s.balance.staked == 10_000_000,
    )
    .await;
    assert!(s.roster.iter().any(|r| r.finalizer == finalizer && r.ours));

    let since = s.bonds[0].since;
    let at = s.bonds[0].next_action_at(node_tip());
    eprintln!("  bond since {since}, unbond from {at}");
    generate(at.saturating_sub(1 + node_tip()));
    drive(
        "synced to the unbond",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(1800),
        synced_to(at - 1),
    )
    .await;
    act(
        StakeOp::Unbond {
            bond: s.bonds[0].clone(),
        },
        &mut s,
        &mut act_rx,
        &cmd_tx,
    )
    .await;
    drive(
        "bond unbonding",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(300),
        |s| {
            s.bonds
                .first()
                .is_some_and(|b| b.status == BondStatus::Unbonding && b.since > since)
        },
    )
    .await;

    let at = s.bonds[0].next_action_at(node_tip());
    eprintln!("  unbonded at {}, withdraw from {at}", s.bonds[0].since);
    generate(at.saturating_sub(1 + node_tip()));
    drive(
        "synced to the withdrawal",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(1800),
        synced_to(at - 1),
    )
    .await;
    act(
        StakeOp::Withdraw {
            bond: s.bonds[0].clone(),
        },
        &mut s,
        &mut act_rx,
        &cmd_tx,
    )
    .await;
    drive(
        "bond withdrawn",
        &mut s,
        &mut act_rx,
        &cmd_tx,
        Duration::from_secs(300),
        |s| s.bonds.is_empty() && s.balance.unbonding == 0,
    )
    .await;
    eprintln!(
        "PASS: bonded, unbonded and withdrew at block {}",
        node_tip()
    );
}
