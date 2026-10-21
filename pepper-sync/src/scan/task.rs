use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
    time::Duration,
};

use futures::FutureExt;
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::{self, BlockHeight};
use zip32::AccountId;

use crate::{
    client::{self, FetchRequest, IronwoodSizes},
    config::PerformanceLevel,
    error::{ScanError, ServerError, SyncError},
    keys::transparent::TransparentAddressId,
    sync::{self, ScanPriority, ScanRange},
    wallet::{
        ScanTarget, TreeBounds, WalletBlock,
        traits::{SyncBlocks, SyncNullifiers, SyncWallet},
    },
};

use super::{ScanResults, compact_blocks, scan};

const MAX_WORKER_POOLSIZE: usize = 2;
const MAX_BATCH_NULLIFIERS: usize = 2usize.pow(14);

const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_MSG_TIMEOUT: Duration = Duration::from_secs(15);
const SCAN_TASK_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) enum ScannerState {
    Verification,
    Scan,
    Shutdown,
}

impl ScannerState {
    fn verified(&mut self) {
        *self = ScannerState::Scan;
    }

    fn shutdown(&mut self) {
        *self = ScannerState::Shutdown;
    }
}

pub(crate) struct Scanner<P> {
    pub(crate) state: ScannerState,
    batcher: Option<Batcher<P>>,
    workers: Vec<ScanWorker<P>>,
    unique_id: usize,
    next_task_id: TaskId,
    in_flight_tasks: BTreeMap<TaskId, ScanRange>,
    load_order: LoadOrder,
    scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ironwood_sizes: IronwoodSizes,
    consensus_parameters: P,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

impl<P> Scanner<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    pub(crate) fn new(
        consensus_parameters: P,
        scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ironwood_sizes: IronwoodSizes,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        let workers: Vec<ScanWorker<P>> = Vec::with_capacity(MAX_WORKER_POOLSIZE);

        Self {
            state: ScannerState::Verification,
            batcher: None,
            workers,
            unique_id: 0,
            next_task_id: TaskId::first(),
            in_flight_tasks: BTreeMap::new(),
            load_order: LoadOrder::default(),
            scan_results_sender,
            fetch_request_sender,
            ironwood_sizes,
            consensus_parameters,
            ufvks,
        }
    }

    pub(crate) fn launch(&mut self, performance_level: PerformanceLevel) {
        let max_batch_outputs = match performance_level {
            PerformanceLevel::Low => 2usize.pow(11),
            PerformanceLevel::Medium => 2usize.pow(13),
            PerformanceLevel::High => 2usize.pow(13),
            PerformanceLevel::Maximum => 2usize.pow(15),
        };

        self.spawn_batcher(max_batch_outputs);
        self.spawn_workers(max_batch_outputs);
    }

    pub(crate) fn worker_poolsize(&self) -> usize {
        self.workers.len()
    }

    /// Forgets every task whose selected range no longer overlaps a wallet range held at its in-flight priority,
    /// with any results of its loads held back for ordering.
    pub(crate) fn retire_finished_tasks(&mut self, scan_ranges: &[ScanRange]) {
        self.in_flight_tasks.retain(|_, task| {
            sync::held_at(scan_ranges, task.priority().in_flight())
                .any(|held| held.overlaps(task.block_range()))
        });
        self.load_order.retain(&self.in_flight_tasks);
    }

    /// - Holds the results of a load above an unprocessed lower load of its task back until that load arrives.
    pub(crate) fn admit_scan_results(
        &mut self,
        load: ScanLoad,
        scan_results: Result<ScanResults, ScanError>,
    ) -> Vec<(ScanLoad, Result<ScanResults, ScanError>)> {
        self.load_order.admit(load, scan_results)
    }

    /// Spawns the batcher.
    ///
    /// When the batcher is running it will wait for a scan task.
    pub(crate) fn spawn_batcher(&mut self, max_batch_outputs: usize) {
        tracing::debug!("Spawning batcher");
        let mut batcher = Batcher::new(
            self.consensus_parameters.clone(),
            self.fetch_request_sender.clone(),
            self.ironwood_sizes.clone(),
        );
        batcher.run(max_batch_outputs);
        self.batcher = Some(batcher);
    }

    fn check_batcher_error(&mut self) -> Result<(), ServerError> {
        let batcher = self.batcher.take();
        if let Some(mut batcher) = batcher {
            batcher.check_error()?;
            self.batcher = Some(batcher);
        }

        Ok(())
    }

    async fn shutdown_batcher(&mut self) -> Result<(), ServerError> {
        let batcher = self.batcher.take();
        if let Some(mut batcher) = batcher {
            batcher.shutdown().await
        } else {
            Ok(())
        }
    }

    /// Spawns a worker.
    ///
    /// When the worker is running it will wait for a scan task.
    pub(crate) fn spawn_worker(&mut self, max_batch_outputs: usize) {
        tracing::debug!("Spawning worker {}", self.unique_id);
        let mut worker = ScanWorker::new(
            self.unique_id,
            self.consensus_parameters.clone(),
            self.scan_results_sender.clone(),
            self.fetch_request_sender.clone(),
            self.ironwood_sizes.clone(),
            self.ufvks.clone(),
        );
        worker.run(max_batch_outputs);
        self.workers.push(worker);
        self.unique_id += 1;
    }

    /// Spawns the initial pool of workers.
    ///
    /// Poolsize is set by [`self::MAX_WORKER_POOLSIZE`].
    pub(crate) fn spawn_workers(&mut self, max_batch_outputs: usize) {
        for _ in 0..MAX_WORKER_POOLSIZE {
            self.spawn_worker(max_batch_outputs);
        }
    }

    fn idle_worker(&self) -> Option<&ScanWorker<P>> {
        if let Some(idle_worker) = self.workers.iter().find(|worker| !worker.is_scanning()) {
            Some(idle_worker)
        } else {
            None
        }
    }

    /// Shutdown worker by `worker_id`.
    ///
    /// Panics if worker with given `worker_id` is not found.
    async fn shutdown_worker(&mut self, worker_id: usize) {
        let worker_index = self
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
            .expect("worker should exist");

        let mut worker = self.workers.swap_remove(worker_index);
        worker.shutdown().await.expect("worker task panicked");
    }

    /// Updates the scanner.
    ///
    /// Creates a new scan task and sends to batcher if it's idle.
    /// The batcher will stream compact blocks into the scan task, splitting the scan task when the maximum number of
    /// outputs is reached. When a scan task is ready it is stored in the batcher ready to be taken by an idle scan
    /// worker for scanning.
    /// When verification is still in progress, only scan tasks with `Verify` scan priority are created.
    /// When all ranges are scanned, the batcher, idle workers and mempool are shutdown.
    pub(crate) async fn update<W>(
        &mut self,
        wallet: &mut W,
        shutdown_mempool: Arc<AtomicBool>,
        nullifier_map_limit_exceeded: bool,
    ) -> Result<(), SyncError<W::Error>>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers,
    {
        self.check_batcher_error()?;

        match self.state {
            ScannerState::Verification => {
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .update_batch_store();
                self.update_workers();

                let sync_state = wallet.get_sync_state().map_err(SyncError::WalletError)?;
                if !sync_state
                    .scan_ranges()
                    .iter()
                    .any(|scan_range| scan_range.priority() == ScanPriority::Verify)
                {
                    if sync_state
                        .scan_ranges()
                        .iter()
                        .any(|scan_range| scan_range.priority() == ScanPriority::Scanning)
                    {
                        // the last scan ranges with `Verify` priority are currently being scanned.
                        return Ok(());
                    }
                    // verification complete
                    self.state.verified();
                    return Ok(());
                }

                // scan ranges with `Verify` priority
                self.update_batcher(wallet, nullifier_map_limit_exceeded)
                    .map_err(SyncError::WalletError)?;
            }
            ScannerState::Scan => {
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .update_batch_store();
                self.update_workers();
                self.update_batcher(wallet, nullifier_map_limit_exceeded)
                    .map_err(SyncError::WalletError)?;
            }
            ScannerState::Shutdown => {
                shutdown_mempool.store(true, atomic::Ordering::Release);
                while let Some(worker) = self.idle_worker() {
                    self.shutdown_worker(worker.id).await;
                }
                self.shutdown_batcher().await?;
            }
        }

        Ok(())
    }

    fn update_workers(&mut self) {
        let batcher = self.batcher.as_ref().expect("batcher should be running");
        let scanning_workers = self
            .workers
            .iter()
            .filter(|worker| worker.is_scanning())
            .count();
        if batcher.batch.is_some()
            && load_admitted(scanning_workers, self.load_order.held())
            && let Some(worker) = self.idle_worker()
        {
            let batch = batcher
                .batch
                .clone()
                .expect("batch should exist in this closure");
            worker.add_scan_task(batch);
            self.batcher
                .as_mut()
                .expect("batcher should be running")
                .batch = None;
        }
    }

    fn update_batcher<W>(
        &mut self,
        wallet: &mut W,
        nullifier_map_limit_exceeded: bool,
    ) -> Result<(), W::Error>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers,
    {
        let batcher = self.batcher.as_ref().expect("batcher should be running");
        if !batcher.is_batching() {
            if let Some(scan_task) = sync::state::create_scan_task(
                &self.consensus_parameters,
                wallet,
                nullifier_map_limit_exceeded,
                self.next_task_id,
            )? {
                self.in_flight_tasks
                    .insert(scan_task.task_id, scan_task.scan_range.clone());
                self.load_order
                    .track(scan_task.task_id, scan_task.scan_range.block_range().start);
                self.next_task_id = self.next_task_id.next();
                batcher.add_scan_task(scan_task);
            } else if wallet.get_sync_state()?.scan_complete() {
                self.state.shutdown();
            }
        }

        Ok(())
    }
}

struct Batcher<P> {
    handle: Option<JoinHandle<Result<(), ServerError>>>,
    is_batching: Arc<AtomicBool>,
    batch: Option<ScanTask>,
    consensus_parameters: P,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
    batch_receiver: Option<mpsc::Receiver<ScanTask>>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ironwood_sizes: IronwoodSizes,
}

impl<P> Batcher<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    fn new(
        consensus_parameters: P,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ironwood_sizes: IronwoodSizes,
    ) -> Self {
        Self {
            handle: None,
            is_batching: Arc::new(AtomicBool::new(false)),
            batch: None,
            consensus_parameters,
            scan_task_sender: None,
            batch_receiver: None,
            fetch_request_sender,
            ironwood_sizes,
        }
    }

    /// Runs the batcher in a new tokio task.
    ///
    /// Waits for a scan task and then fetches compact blocks to form fixed output batches. The scan task is split if
    /// needed and the compact blocks are added to each scan task and sent to the scan workers for scanning.
    fn run(&mut self, max_batch_outputs: usize) {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);
        let (batch_sender, batch_receiver) = mpsc::channel::<ScanTask>(1);

        let is_batching = self.is_batching.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();
        let ironwood_sizes = self.ironwood_sizes.clone();
        let consensus_parameters = self.consensus_parameters.clone();

        let handle: JoinHandle<Result<(), ServerError>> = tokio::spawn(async move {
            // save seam blocks between scan tasks for linear scanning continuity checks
            // during non-linear scanning the wallet blocks from the scanned ranges will already be saved in the wallet
            let mut previous_task_first_block: Option<WalletBlock> = None;
            let mut previous_task_last_block: Option<WalletBlock> = None;

            while let Some(mut scan_task) = scan_task_receiver.recv().await {
                let fetch_nullifiers_only =
                    scan_task.scan_range.priority() == ScanPriority::ScannedWithoutMapping;

                let mut retry_height = scan_task.scan_range.block_range().start;
                let mut sapling_output_count = 0;
                let mut orchard_output_count = 0;
                let mut ironwood_output_count = 0;
                let mut sapling_nullifier_count = 0;
                let mut orchard_nullifier_count = 0;
                let mut ironwood_nullifier_count = 0;
                let mut first_batch = true;
                // final tree sizes of the block before the one being batched, once known
                let mut previous_block_final_tree_sizes: Option<(u32, u32, u32)> = None;

                let mut block_stream = {
                    let range = scan_task.scan_range.block_range().clone();
                    let frs = fetch_request_sender.clone();

                    let open_fut = async move {
                        if fetch_nullifiers_only {
                            client::get_nullifier_range(frs, range).await
                        } else {
                            client::get_compact_block_range(frs, range).await
                        }
                    };

                    match tokio::time::timeout(STREAM_OPEN_TIMEOUT, open_fut).await {
                        Ok(res) => res?,
                        Err(_) => {
                            return Err(
                                tonic::Status::deadline_exceeded("open stream timeout").into()
                            );
                        }
                    }
                };

                loop {
                    let msg_res: Result<Option<CompactBlock>, tonic::Status> =
                        match tokio::time::timeout(STREAM_MSG_TIMEOUT, block_stream.message()).await
                        {
                            Ok(res) => res,
                            Err(_) => {
                                Err(tonic::Status::deadline_exceeded("stream message timeout"))
                            }
                        };

                    let maybe_block = match msg_res {
                        Ok(b) => b,
                        Err(e)
                            if e.code() == tonic::Code::DeadlineExceeded
                                || e.message().contains("Unexpected EOF decoding stream.") =>
                        {
                            tokio::time::sleep(Duration::from_secs(3)).await;

                            let retry_range = retry_height..scan_task.scan_range.block_range().end;

                            let reopen_fut = {
                                let frs = fetch_request_sender.clone();

                                async move {
                                    if fetch_nullifiers_only {
                                        client::get_nullifier_range(frs, retry_range).await
                                    } else {
                                        client::get_compact_block_range(frs, retry_range).await
                                    }
                                }
                            };

                            block_stream =
                                match tokio::time::timeout(STREAM_OPEN_TIMEOUT, reopen_fut).await {
                                    Ok(res) => res?,
                                    Err(_) => {
                                        return Err(tonic::Status::deadline_exceeded(
                                            "open stream timeout (retry)",
                                        )
                                        .into());
                                    }
                                };

                            let first_msg_res: Result<Option<CompactBlock>, tonic::Status> =
                                match tokio::time::timeout(
                                    STREAM_MSG_TIMEOUT,
                                    block_stream.message(),
                                )
                                .await
                                {
                                    Ok(res) => res,
                                    Err(_) => Err(tonic::Status::deadline_exceeded(
                                        "stream message timeout after retry",
                                    )),
                                };

                            match first_msg_res {
                                Ok(b) => b,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        Err(e) => {
                            return Err(e.into());
                        }
                    };

                    let Some(compact_block) = maybe_block else {
                        break;
                    };

                    if fetch_nullifiers_only {
                        sapling_nullifier_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.spends.len());
                        orchard_nullifier_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.actions.len());
                        ironwood_nullifier_count +=
                            compact_block.vtx.iter().fold(0, |acc, transaction| {
                                acc + transaction.ironwood_actions.len()
                            });
                    } else {
                        if let Some(block) = previous_task_last_block.as_ref()
                            && scan_task.start_seam_block.is_none()
                            && scan_task.scan_range.block_range().start == block.block_height() + 1
                        {
                            scan_task.start_seam_block = previous_task_last_block.clone();
                        }
                        if let Some(block) = previous_task_first_block.as_ref()
                            && scan_task.end_seam_block.is_none()
                            && scan_task.scan_range.block_range().end == block.block_height()
                        {
                            scan_task.end_seam_block = previous_task_first_block.clone();
                        }
                        // the tree bounds of each block follow from the block before it, so the server is only
                        // asked for the first block of a range without a start seam block. every block's Ironwood
                        // size is remembered for servers that serve no Ironwood tree state.
                        let tree_bounds = match previous_block_final_tree_sizes.or_else(|| {
                            scan_task
                                .start_seam_block
                                .as_ref()
                                .map(|block| block.tree_bounds().final_tree_sizes())
                        }) {
                            Some(initial_tree_sizes) => TreeBounds::from_initial_tree_sizes(
                                &compact_block,
                                initial_tree_sizes,
                            ),
                            None => {
                                compact_blocks::calculate_block_tree_bounds(
                                    &consensus_parameters,
                                    fetch_request_sender.clone(),
                                    &ironwood_sizes,
                                    &compact_block,
                                )
                                .await?
                            }
                        };
                        ironwood_sizes
                            .record(compact_block.height(), tree_bounds.ironwood_final_tree_size);
                        previous_block_final_tree_sizes = Some(tree_bounds.final_tree_sizes());

                        if first_batch {
                            previous_task_first_block =
                                Some(WalletBlock::with_tree_bounds(&compact_block, tree_bounds));
                            first_batch = false;
                        }
                        if compact_block.height() == scan_task.scan_range.block_range().end - 1 {
                            previous_task_last_block =
                                Some(WalletBlock::with_tree_bounds(&compact_block, tree_bounds));
                        }

                        sapling_output_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.outputs.len());
                        orchard_output_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.actions.len());
                        ironwood_output_count +=
                            compact_block.vtx.iter().fold(0, |acc, transaction| {
                                acc + transaction.ironwood_actions.len()
                            });
                    }

                    if sapling_output_count + orchard_output_count + ironwood_output_count
                        > max_batch_outputs
                        || sapling_nullifier_count
                            + orchard_nullifier_count
                            + ironwood_nullifier_count
                            > MAX_BATCH_NULLIFIERS
                    {
                        let (full_batch, new_batch) = scan_task
                            .clone()
                            .split(
                                &consensus_parameters,
                                fetch_request_sender.clone(),
                                &ironwood_sizes,
                                compact_block.height(),
                            )
                            .await?;

                        let _ignore_error = batch_sender.send(full_batch).await;

                        scan_task = new_batch;
                        sapling_output_count = 0;
                        orchard_output_count = 0;
                        ironwood_output_count = 0;
                        sapling_nullifier_count = 0;
                        orchard_nullifier_count = 0;
                        ironwood_nullifier_count = 0;
                    }

                    retry_height = compact_block.height() + 1;
                    scan_task.compact_blocks.push(compact_block);
                }

                let _ignore_error = batch_sender.send(scan_task).await;

                is_batching.store(false, atomic::Ordering::Release);
            }
            Ok(())
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
        self.batch_receiver = Some(batch_receiver);
    }

    fn is_batching(&self) -> bool {
        self.is_batching.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to batcher:\n{:#?}", &scan_task);
        self.scan_task_sender
            .clone()
            .expect("batcher should be running")
            .try_send(scan_task)
            .expect("batcher should never be sent multiple tasks at one time");
        self.is_batching.store(true, atomic::Ordering::Release);
    }

    fn update_batch_store(&mut self) {
        let batch_receiver = self
            .batch_receiver
            .as_mut()
            .expect("batcher should be running");
        if self.batch.is_none() && !batch_receiver.is_empty() {
            self.batch = Some(
                batch_receiver
                    .try_recv()
                    .expect("channel should be non-empty!"),
            );
        }
    }

    fn check_error(&mut self) -> Result<(), ServerError> {
        if let Some(mut handle) = self.handle.take() {
            if let Some(result) = handle.borrow_mut().now_or_never() {
                result.expect("task panicked")?;
            } else {
                self.handle = Some(handle);
            }
        }

        Ok(())
    }

    /// Shuts down batcher by dropping the sender to the batcher task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be taken from the Scanner struct.
    async fn shutdown(&mut self) -> Result<(), ServerError> {
        tracing::debug!("Shutting down batcher");
        if let Some(sender) = self.scan_task_sender.take() {
            drop(sender);
        }
        if let Some(receiver) = self.batch_receiver.take() {
            drop(receiver);
        }

        let mut handle = self
            .handle
            .take()
            .expect("batcher should always have a handle to take!");

        match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(join_res) => join_res.expect("task panicked")?,
            Err(_) => {
                handle.abort();
                let _ = handle.await;
                return Err(tonic::Status::deadline_exceeded("batcher shutdown timeout").into());
            }
        }

        Ok(())
    }
}

pub(crate) struct ScanWorker<P> {
    id: usize,
    handle: Option<JoinHandle<()>>,
    is_scanning: Arc<AtomicBool>,
    consensus_parameters: P,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
    scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ironwood_sizes: IronwoodSizes,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

impl<P> ScanWorker<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    fn new(
        id: usize,
        consensus_parameters: P,
        scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ironwood_sizes: IronwoodSizes,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        Self {
            id,
            handle: None,
            is_scanning: Arc::new(AtomicBool::new(false)),
            consensus_parameters,
            scan_task_sender: None,
            scan_results_sender,
            fetch_request_sender,
            ironwood_sizes,
            ufvks,
        }
    }

    /// Runs the worker in a new tokio task.
    ///
    /// Waits for a scan task and then calls [`crate::scan::scan`] on the given range.
    fn run(&mut self, max_batch_outputs: usize) {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);

        let is_scanning = self.is_scanning.clone();
        let scan_results_sender = self.scan_results_sender.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();
        let ironwood_sizes = self.ironwood_sizes.clone();
        let consensus_parameters = self.consensus_parameters.clone();
        let ufvks = self.ufvks.clone();

        let handle = tokio::spawn(async move {
            while let Some(scan_task) = scan_task_receiver.recv().await {
                let load = ScanLoad {
                    task_id: scan_task.task_id,
                    scan_range: scan_task.scan_range.clone(),
                };

                let scan_fut = scan(
                    fetch_request_sender.clone(),
                    &consensus_parameters,
                    &ironwood_sizes,
                    &ufvks,
                    scan_task,
                    max_batch_outputs,
                );

                let scan_results = match tokio::time::timeout(SCAN_TASK_TIMEOUT, scan_fut).await {
                    Ok(res) => res,
                    Err(_) => {
                        // Best-effort: maps timeout into existing error types.
                        Err(ServerError::from(tonic::Status::deadline_exceeded(
                            "scan task timeout",
                        ))
                        .into())
                    }
                };

                let _ignore_error = scan_results_sender.send((load, scan_results));

                is_scanning.store(false, atomic::Ordering::Release);
            }
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
    }

    fn is_scanning(&self) -> bool {
        self.is_scanning.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to worker {}:\n{:#?}", self.id, &scan_task);
        self.scan_task_sender
            .clone()
            .expect("worker should be running")
            .try_send(scan_task)
            .expect("worker should never be sent multiple tasks at one time");
        self.is_scanning.store(true, atomic::Ordering::Release);
    }

    /// Shuts down worker by dropping the sender to the worker task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be removed from the worker pool.
    async fn shutdown(&mut self) -> Result<(), JoinError> {
        tracing::debug!("Shutting down worker {}", self.id);
        if let Some(sender) = self.scan_task_sender.take() {
            drop(sender);
        }

        let mut handle = self
            .handle
            .take()
            .expect("worker should always have a handle to take!");

        match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(res) => res,
            Err(_) => {
                handle.abort();
                let _ = handle.await; // ignore join error after abort
                Ok(())
            }
        }
    }
}

/// The identity of a scan task, unique within a scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TaskId(u64);

impl TaskId {
    /// Returns the identity of the first task a scanner selects.
    pub(crate) fn first() -> Self {
        TaskId(0)
    }

    /// Returns the identity of the task selected after this one.
    pub(crate) fn next(self) -> Self {
        TaskId(self.0 + 1)
    }
}

/// A load of a scan task: the range of blocks a worker scans in one go, and the task it belongs to.
#[derive(Debug, Clone)]
pub(crate) struct ScanLoad {
    pub(crate) task_id: TaskId,
    pub(crate) scan_range: ScanRange,
}

// a held result occupies the place of the compact blocks its lower load's worker still holds, so the loads in
// memory never exceed the worker pool.
fn load_admitted(scanning_workers: usize, held_results: usize) -> bool {
    scanning_workers + held_results < MAX_WORKER_POOLSIZE
}

// the loads of a task are built in block height order and their results return in completion order. a task's
// results are held back until every lower load of the task has been released, so a note found in a lower load is
// known to the wallet before the spends of any higher load are located.
#[derive(Default)]
struct LoadOrder {
    // the start of the load each tracked task releases next.
    next_load_start: BTreeMap<TaskId, BlockHeight>,
    // results of loads above their task's next load, keyed by task and load start.
    held: BTreeMap<(TaskId, BlockHeight), (ScanLoad, Result<ScanResults, ScanError>)>,
}

impl LoadOrder {
    /// - Tracks `task_id` from `first_load_start`.
    fn track(&mut self, task_id: TaskId, first_load_start: BlockHeight) {
        self.next_load_start.insert(task_id, first_load_start);
    }

    /// - Holds `scan_results` back when `load` is above its task's next load.
    /// - Otherwise releases it and every held load that then follows it, advancing the task's next load past each.
    ///
    /// The results of an untracked task are released as they are.
    fn admit(
        &mut self,
        load: ScanLoad,
        scan_results: Result<ScanResults, ScanError>,
    ) -> Vec<(ScanLoad, Result<ScanResults, ScanError>)> {
        let task_id = load.task_id;
        if !self.next_load_start.contains_key(&task_id) {
            return vec![(load, scan_results)];
        }
        self.held.insert(
            (task_id, load.scan_range.block_range().start),
            (load, scan_results),
        );

        let mut released = Vec::new();
        while let Some(next_load_start) = self.next_load_start.get_mut(&task_id)
            && let Some((load, scan_results)) = self.held.remove(&(task_id, *next_load_start))
        {
            *next_load_start = load.scan_range.block_range().end;
            released.push((load, scan_results));
        }

        released
    }

    fn held(&self) -> usize {
        self.held.len()
    }

    /// - Drops the tracking and held results of every task absent from `in_flight_tasks`.
    fn retain(&mut self, in_flight_tasks: &BTreeMap<TaskId, ScanRange>) {
        self.next_load_start
            .retain(|task_id, _| in_flight_tasks.contains_key(task_id));
        self.held
            .retain(|(task_id, _), _| in_flight_tasks.contains_key(task_id));
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ScanTask {
    pub(crate) task_id: TaskId,
    pub(crate) compact_blocks: Vec<CompactBlock>,
    pub(crate) scan_range: ScanRange,
    pub(crate) start_seam_block: Option<WalletBlock>,
    pub(crate) end_seam_block: Option<WalletBlock>,
    pub(crate) scan_targets: BTreeSet<ScanTarget>,
    pub(crate) transparent_addresses: HashMap<String, TransparentAddressId>,
}

impl ScanTask {
    pub(crate) fn from_parts(
        task_id: TaskId,
        scan_range: ScanRange,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
        scan_targets: BTreeSet<ScanTarget>,
        transparent_addresses: HashMap<String, TransparentAddressId>,
    ) -> Self {
        Self {
            task_id,
            compact_blocks: Vec::new(),
            scan_range,
            start_seam_block,
            end_seam_block,
            scan_targets,
            transparent_addresses,
        }
    }

    /// Splits a scan task into two at `block_height`.
    ///
    /// Panics if `block_height` is not contained in the scan task's block range.
    async fn split(
        self,
        consensus_parameters: &impl consensus::Parameters,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ironwood_sizes: &IronwoodSizes,
        block_height: BlockHeight,
    ) -> Result<(Self, Self), ServerError> {
        if block_height < self.scan_range.block_range().start
            && block_height > self.scan_range.block_range().end - 1
        {
            panic!("block height should be within scan tasks block range!");
        }

        let mut lower_compact_blocks = self.compact_blocks;
        let upper_compact_blocks = if let Some(index) = lower_compact_blocks
            .iter()
            .position(|block| block.height() == block_height)
        {
            lower_compact_blocks.split_off(index)
        } else {
            Vec::new()
        };

        let mut lower_task_scan_targets = self.scan_targets;
        let upper_task_scan_targets = lower_task_scan_targets.split_off(&ScanTarget {
            block_height,
            txid: TxId::from_bytes([0; 32]),
            narrow_scan_area: false,
        });

        // the seam blocks follow from the blocks at hand, so the server is only asked where the lower task has no
        // start seam block.
        let lower_task_last_block = tree_bounds_at_ends(
            consensus_parameters,
            fetch_request_sender.clone(),
            ironwood_sizes,
            self.start_seam_block.as_ref(),
            &lower_compact_blocks,
        )
        .await?
        .map(|(_, last_tree_bounds)| {
            WalletBlock::with_tree_bounds(
                lower_compact_blocks.last().expect("bounds imply a block"),
                last_tree_bounds,
            )
        });
        let upper_task_first_block = tree_bounds_at_ends(
            consensus_parameters,
            fetch_request_sender,
            ironwood_sizes,
            lower_task_last_block
                .as_ref()
                .or(self.start_seam_block.as_ref()),
            &upper_compact_blocks,
        )
        .await?
        .map(|(first_tree_bounds, _)| {
            WalletBlock::with_tree_bounds(
                upper_compact_blocks.first().expect("bounds imply a block"),
                first_tree_bounds,
            )
        });

        Ok((
            ScanTask {
                task_id: self.task_id,
                compact_blocks: lower_compact_blocks,
                scan_range: self
                    .scan_range
                    .truncate_end(block_height)
                    .expect("block height should be within block range"),
                start_seam_block: self.start_seam_block,
                end_seam_block: upper_task_first_block,
                scan_targets: lower_task_scan_targets,
                transparent_addresses: self.transparent_addresses.clone(),
            },
            ScanTask {
                task_id: self.task_id,
                compact_blocks: upper_compact_blocks,
                scan_range: self
                    .scan_range
                    .truncate_start(block_height)
                    .expect("block height should be within block range"),
                start_seam_block: lower_task_last_block,
                end_seam_block: self.end_seam_block,
                scan_targets: upper_task_scan_targets,
                transparent_addresses: self.transparent_addresses,
            },
        ))
    }
}

/// The tree bounds of the first and last blocks of `blocks`, each block's bounds following from the block before
/// it: from `preceding` (the block before `blocks[0]`) when known, and from the server for the first block
/// otherwise. `None` if `blocks` is empty.
async fn tree_bounds_at_ends(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ironwood_sizes: &IronwoodSizes,
    preceding: Option<&WalletBlock>,
    blocks: &[CompactBlock],
) -> Result<Option<(TreeBounds, TreeBounds)>, ServerError> {
    let Some(first) = blocks.first() else {
        return Ok(None);
    };
    let first_tree_bounds = match preceding {
        Some(block) => {
            TreeBounds::from_initial_tree_sizes(first, block.tree_bounds().final_tree_sizes())
        }
        None => {
            compact_blocks::calculate_block_tree_bounds(
                consensus_parameters,
                fetch_request_sender,
                ironwood_sizes,
                first,
            )
            .await?
        }
    };
    let last_tree_bounds = blocks[1..]
        .iter()
        .fold(first_tree_bounds, |previous, block| {
            TreeBounds::from_initial_tree_sizes(block, previous.final_tree_sizes())
        });

    Ok(Some((first_tree_bounds, last_tree_bounds)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(task_id: TaskId, start: u32, end: u32) -> ScanLoad {
        ScanLoad {
            task_id,
            scan_range: ScanRange::from_parts(
                BlockHeight::from_u32(start)..BlockHeight::from_u32(end),
                ScanPriority::ChainTip,
            ),
        }
    }

    fn results() -> Result<ScanResults, ScanError> {
        Err(ScanError::ServerError(ServerError::RequestFailed(
            tonic::Status::not_found("block"),
        )))
    }

    fn released_starts(released: &[(ScanLoad, Result<ScanResults, ScanError>)]) -> Vec<u32> {
        released
            .iter()
            .map(|(load, _)| u32::from(load.scan_range.block_range().start))
            .collect()
    }

    /// Loads of a task arriving in block height order are released as they arrive.
    #[test]
    fn loads_arriving_in_order_are_released_as_they_arrive() {
        let task = TaskId::first();
        let mut order = LoadOrder::default();
        order.track(task, BlockHeight::from_u32(10));

        assert_eq!(
            released_starts(&order.admit(load(task, 10, 20), results())),
            [10]
        );
        assert_eq!(
            released_starts(&order.admit(load(task, 20, 30), results())),
            [20]
        );
        assert_eq!(order.held(), 0);
    }

    /// A load arriving above an unreleased lower load of its task is held back, and released in height order once
    /// the lower load arrives.
    #[test]
    fn a_load_above_an_unreleased_load_is_held_until_the_lower_load_arrives() {
        let task = TaskId::first();
        let mut order = LoadOrder::default();
        order.track(task, BlockHeight::from_u32(10));

        assert!(order.admit(load(task, 30, 40), results()).is_empty());
        assert!(order.admit(load(task, 20, 30), results()).is_empty());
        assert_eq!(order.held(), 2);
        assert_eq!(
            released_starts(&order.admit(load(task, 10, 20), results())),
            [10, 20, 30]
        );
        assert_eq!(order.held(), 0);
    }

    /// The ordering of one task does not hold back the loads of another.
    #[test]
    fn tasks_are_ordered_independently() {
        let first = TaskId::first();
        let second = first.next();
        let mut order = LoadOrder::default();
        order.track(first, BlockHeight::from_u32(10));
        order.track(second, BlockHeight::from_u32(50));

        assert!(order.admit(load(first, 20, 30), results()).is_empty());
        assert_eq!(
            released_starts(&order.admit(load(second, 50, 60), results())),
            [50]
        );
    }

    /// The results of a task the scanner does not track are released as they are.
    #[test]
    fn loads_of_an_untracked_task_are_released_as_they_are() {
        let mut order = LoadOrder::default();

        assert_eq!(
            released_starts(&order.admit(load(TaskId::first(), 20, 30), results())),
            [20]
        );
        assert_eq!(order.held(), 0);
    }

    /// Retiring a task drops the results held back for it.
    #[test]
    fn retiring_a_task_drops_its_held_results() {
        let task = TaskId::first();
        let mut order = LoadOrder::default();
        order.track(task, BlockHeight::from_u32(10));
        assert!(order.admit(load(task, 20, 30), results()).is_empty());

        order.retain(&BTreeMap::new());

        assert_eq!(order.held(), 0);
        assert_eq!(
            released_starts(&order.admit(load(task, 10, 20), results())),
            [10]
        );
    }

    /// A held result takes the place of a load of compact blocks in a worker, so a worker takes a load only while
    /// the scanning workers and held results together stay within the pool.
    #[test]
    fn a_worker_takes_a_load_only_within_the_pool() {
        for (scanning_workers, held_results, admitted) in [
            (0, 0, true),
            (MAX_WORKER_POOLSIZE - 1, 0, true),
            (0, MAX_WORKER_POOLSIZE - 1, true),
            (MAX_WORKER_POOLSIZE - 1, 1, false),
            (0, MAX_WORKER_POOLSIZE, false),
        ] {
            assert_eq!(
                load_admitted(scanning_workers, held_results),
                admitted,
                "{scanning_workers} scanning, {held_results} held"
            );
        }
    }
}
