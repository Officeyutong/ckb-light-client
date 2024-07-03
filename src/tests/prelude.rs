use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use ckb_chain::chain::ChainController;
use ckb_chain_spec::consensus::Consensus;
use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_shared::{Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_tx_pool::TxPoolController;
use ckb_types::{
    core::{BlockExt, BlockNumber, BlockView, Capacity, HeaderView, TransactionView},
    packed,
    prelude::*,
    utilities::{
        build_filter_data, calc_filter_hash, compact_to_difficulty,
        merkle_mountain_range::VerifiableHeader, FilterDataProvider,
    },
    U256,
};
use log::{error, info};

use crate::{
    protocols::{
        FilterProtocol, LastState, LightClientProtocol, Peers, ProveRequest, SyncProtocol,
        CHECK_POINT_INTERVAL,
    },
    storage::Storage,
    tests::{ALWAYS_SUCCESS_BIN, ALWAYS_SUCCESS_SCRIPT},
};

macro_rules! epoch {
    ($number:expr, $index:expr, $length:expr) => {
        ckb_types::core::EpochNumberWithFraction::new($number, $index, $length)
    };
    ($tuple:ident) => {{
        let (number, index, length) = $tuple;
        ckb_types::core::EpochNumberWithFraction::new(number, index, length)
    }};
}

struct FilterData {
    inner: HashMap<packed::Byte32, TransactionView>,
}

impl FilterData {
    fn new(genesis: &BlockView) -> Self {
        let inner = genesis
            .transactions()
            .into_iter()
            .map(|tx| (tx.hash(), tx))
            .collect();
        Self { inner }
    }

    fn extend(&mut self, block: &BlockView) {
        let iter = block.transactions().into_iter().map(|tx| (tx.hash(), tx));
        self.inner.extend(iter);
    }
}

impl FilterDataProvider for &FilterData {
    fn cell(&self, out_point: &packed::OutPoint) -> Option<packed::CellOutput> {
        self.inner
            .get(&out_point.tx_hash())
            .and_then(|tx| tx.outputs().get(out_point.index().unpack()))
    }
}

pub(crate) trait SnapshotExt {
    fn get_header_by_number(&self, num: BlockNumber) -> Option<HeaderView>;

    fn get_block_by_number(&self, num: BlockNumber) -> Option<BlockView>;

    fn get_block_ext_by_number(&self, num: BlockNumber) -> Option<BlockExt>;

    fn get_verifiable_header_by_number(&self, num: BlockNumber)
        -> Option<packed::VerifiableHeader>;

    fn get_block_difficulty_by_number(&self, num: BlockNumber) -> Option<U256> {
        self.get_header_by_number(num)
            .map(|header| compact_to_difficulty(header.compact_target()))
    }

    fn get_total_difficulty_by_number(&self, num: BlockNumber) -> Option<U256> {
        self.get_block_ext_by_number(num)
            .map(|block_ext| block_ext.total_difficulty)
    }

    fn build_last_state_by_number(&self, num: BlockNumber) -> Option<packed::LightClientMessage> {
        self.get_verifiable_header_by_number(num).map(|header| {
            let content = packed::SendLastState::new_builder()
                .last_header(header)
                .build();
            packed::LightClientMessage::new_builder()
                .set(content)
                .build()
        })
    }

    fn get_block_filter_data(&self, num: BlockNumber) -> Option<packed::Bytes> {
        (0..num)
            .try_fold(None::<FilterData>, |provider_opt, curr_num| {
                self.get_block_by_number(curr_num).map(|block_view| {
                    let provider = if let Some(mut provider) = provider_opt {
                        provider.extend(&block_view);
                        provider
                    } else {
                        FilterData::new(&block_view)
                    };
                    Some(provider)
                })
            })
            .and_then(|provider_opt| {
                let mut provider = provider_opt.unwrap();
                self.get_block_by_number(num).map(|block_view| {
                    provider.extend(&block_view);
                    let (block_filter_vec, missing_out_points) =
                        build_filter_data(&provider, &block_view.transactions());
                    if !missing_out_points.is_empty() {
                        for out_point in missing_out_points {
                            error!("Can't find input cell for out_point: {out_point:#x}");
                        }
                        panic!("block#{num} has missing out points!");
                    }
                    block_filter_vec.pack()
                })
            })
    }

    fn get_block_filter_hashes_until(&self, num: BlockNumber) -> Option<Vec<packed::Byte32>> {
        (0..=num)
            .try_fold(
                (packed::Byte32::zero(), Vec::new(), None::<FilterData>),
                |(prev_filter_hash, mut filter_hashes, provider_opt), curr_num| {
                    self.get_block_by_number(curr_num).map(|block_view| {
                        let provider = if let Some(mut provider) = provider_opt {
                            provider.extend(&block_view);
                            provider
                        } else {
                            FilterData::new(&block_view)
                        };
                        let (block_filter_vec, missing_out_points) =
                            build_filter_data(&provider, &block_view.transactions());
                        if !missing_out_points.is_empty() {
                            for out_point in missing_out_points {
                                error!("Can't find input cell for out_point: {out_point:#x}");
                            }
                            panic!("block#{curr_num} has missing out points!");
                        }
                        let block_filter_data = block_filter_vec.pack();
                        let filter_hash =
                            calc_filter_hash(&prev_filter_hash, &block_filter_data).pack();
                        info!("block#{curr_num}'s filter hash is {filter_hash:#x}");
                        filter_hashes.push(filter_hash.clone());
                        (filter_hash, filter_hashes, Some(provider))
                    })
                },
            )
            .map(|(_, filter_hashes, _)| filter_hashes)
    }
}

pub(crate) trait ChainExt {
    fn client_storage(&self) -> &Storage;

    fn consensus(&self) -> &Consensus;

    fn create_peers(&self) -> Arc<Peers> {
        let bad_message_allowed_each_hour = 0;
        self.create_peers_with_parameters(bad_message_allowed_each_hour)
    }

    fn create_peers_with_parameters(&self, bad_message_allowed_each_hour: u32) -> Arc<Peers> {
        let max_outbound_peers = 1;
        let peers = Peers::new(
            max_outbound_peers,
            CHECK_POINT_INTERVAL,
            self.client_storage().get_last_check_point(),
            bad_message_allowed_each_hour,
        );
        Arc::new(peers)
    }

    fn create_light_client_protocol(&self, peers: Arc<Peers>) -> LightClientProtocol {
        let storage = self.client_storage().to_owned();
        let consensus = self.consensus().to_owned();
        let mut protocol = LightClientProtocol::new(storage, peers, consensus);
        protocol.set_mmr_activated_epoch(1);
        protocol
    }

    fn create_filter_protocol(&self, peers: Arc<Peers>) -> FilterProtocol {
        let storage = self.client_storage().to_owned();
        FilterProtocol::new(storage, peers)
    }

    fn create_sync_protocol(&self, peers: Arc<Peers>) -> SyncProtocol {
        let storage = self.client_storage().to_owned();
        SyncProtocol::new(storage, peers)
    }
}

pub(crate) trait RunningChainExt: ChainExt {
    fn controller(&self) -> &ChainController;

    fn shared(&self) -> &Shared;

    fn tx_pool(&self) -> &TxPoolController {
        &self.shared().tx_pool_controller()
    }

    fn always_success_cell_dep(&self) -> packed::CellDep {
        self.shared()
            .snapshot()
            .get_block_by_number(0)
            .unwrap()
            .transactions()
            .into_iter()
            .filter_map(|tx| {
                tx.outputs_data()
                    .into_iter()
                    .enumerate()
                    .find(|(_, data)| data.raw_data().as_ref() == ALWAYS_SUCCESS_BIN)
                    .map(|(idx, _)| {
                        let out_point = packed::OutPoint::new_builder()
                            .tx_hash(tx.hash())
                            .index((idx as u32).pack())
                            .build();
                        packed::CellDep::new_builder().out_point(out_point).build()
                    })
            })
            .next()
            .unwrap()
    }

    fn mine_to(&self, block_number: BlockNumber) {
        let chain_tip_number = self.shared().snapshot().tip_number();
        if chain_tip_number < block_number {
            self.mine_blocks((block_number - chain_tip_number) as usize);
        }
    }

    fn mine_to_with<F: FnMut(packed::Block) -> BlockView>(
        &self,
        block_number: BlockNumber,
        builder: F,
    ) {
        let chain_tip_number = self.shared().snapshot().tip_number();
        if chain_tip_number < block_number {
            self.mine_blocks_with((block_number - chain_tip_number) as usize, builder);
        }
    }

    fn mine_block<F: FnMut(packed::Block) -> BlockView>(&self, mut builder: F) -> BlockNumber {
        let block_template = self
            .shared()
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap();
        let block: packed::Block = block_template.into();
        let block = builder(block);
        let block_number = block.number();
        let is_ok = self
            .controller()
            .process_block(Arc::new(block))
            .expect("process block");
        assert!(is_ok, "failed to process block {}", block_number);
        while self
            .tx_pool()
            .get_tx_pool_info()
            .expect("get tx pool info")
            .tip_number
            != block_number
        {}
        block_number
    }

    fn mine_blocks(&self, blocks_count: usize) {
        for _ in 0..blocks_count {
            let _ = self.mine_block(|block| block.as_advanced_builder().build());
        }
    }

    fn mine_blocks_with<F: FnMut(packed::Block) -> BlockView>(
        &self,
        blocks_count: usize,
        mut builder: F,
    ) {
        for _ in 0..blocks_count {
            let _ = self.mine_block(&mut builder);
        }
    }

    fn get_cellbase_as_input(&self, block_number: BlockNumber) -> TransactionView {
        let snapshot = self.shared().snapshot();
        let block = snapshot.get_block_by_number(block_number).unwrap();
        let cellbase = block.transaction(0).unwrap();
        let input = packed::CellInput::new(packed::OutPoint::new(cellbase.hash(), 0), 0);
        let input_capacity: Capacity = cellbase.output(0).unwrap().capacity().unpack();
        let output_capacity = input_capacity.safe_sub(1000u32).unwrap();
        let header_dep = block.hash();
        let output = packed::CellOutput::new_builder()
            .capacity(output_capacity.pack())
            .lock(ALWAYS_SUCCESS_SCRIPT.to_owned())
            .build();
        TransactionView::new_advanced_builder()
            .cell_dep(self.always_success_cell_dep())
            .header_dep(header_dep)
            .input(input)
            .output(output)
            .output_data(Default::default())
            .build()
    }

    fn rollback_to(
        &self,
        target_number: BlockNumber,
        detached_proposal_ids: HashSet<packed::ProposalShortId>,
    ) {
        let snapshot = self.shared().snapshot();

        let chain_tip_number = snapshot.tip_number();

        let mut detached_blocks = VecDeque::default();
        for num in (target_number + 1)..=chain_tip_number {
            let detached_block = snapshot.get_block_by_number(num).unwrap();
            detached_blocks.push_back(detached_block);
        }

        let target_hash = snapshot.get_header_by_number(target_number).unwrap().hash();
        self.controller().truncate(target_hash).unwrap();
        self.tx_pool()
            .update_tx_pool_for_reorg(
                detached_blocks,
                VecDeque::default(),
                detached_proposal_ids,
                Arc::clone(&self.shared().snapshot()),
            )
            .unwrap();

        while self.shared().snapshot().tip_number() != target_number {}
        while self
            .tx_pool()
            .get_tx_pool_info()
            .expect("get tx pool info")
            .tip_number
            != self.shared().snapshot().tip_number()
        {}
    }

    fn build_prove_request(
        &self,
        start_num: BlockNumber,
        last_num: BlockNumber,
        sampled_nums: &[BlockNumber],
        boundary_num: BlockNumber,
        last_n_blocks: BlockNumber,
    ) -> ProveRequest {
        let snapshot = self.shared().snapshot();
        let last_header: VerifiableHeader = snapshot
            .get_verifiable_header_by_number(last_num)
            .expect("block stored")
            .into();
        let content = {
            let start_header = snapshot
                .get_header_by_number(start_num)
                .expect("block stored");
            let difficulties = {
                let u256_one = &U256::from(1u64);
                let total_diffs = (0..last_num)
                    .into_iter()
                    .map(|num| snapshot.get_total_difficulty_by_number(num).unwrap())
                    .collect::<Vec<_>>();
                let mut difficulties = Vec::new();
                for n in sampled_nums {
                    let n = *n as usize;
                    difficulties.push(&total_diffs[n - 1] + u256_one);
                    difficulties.push(&total_diffs[n] - u256_one);
                    difficulties.push(total_diffs[n].to_owned());
                }
                difficulties.sort();
                difficulties.dedup();
                difficulties.into_iter().map(|diff| diff.pack())
            };
            let difficulty_boundary = snapshot
                .get_total_difficulty_by_number(boundary_num)
                .unwrap();
            packed::GetLastStateProof::new_builder()
                .last_hash(last_header.header().hash())
                .start_hash(start_header.hash())
                .start_number(start_header.number().pack())
                .last_n_blocks(last_n_blocks.pack())
                .difficulty_boundary(difficulty_boundary.pack())
                .difficulties(difficulties.pack())
                .build()
        };
        let last_state = LastState::new(last_header);
        ProveRequest::new(last_state, content)
    }

    fn build_blocks_proof_content(
        &self,
        last_num: BlockNumber,
        block_nums: &[BlockNumber],
        missing_block_hashes: &[packed::Byte32],
    ) -> packed::GetBlocksProof {
        let snapshot = self.shared().snapshot();
        let last_header = snapshot
            .get_header_by_number(last_num)
            .expect("block stored");
        let block_hashes = block_nums
            .iter()
            .map(|n| {
                snapshot
                    .get_header_by_number(*n)
                    .expect("block stored")
                    .hash()
            })
            .chain(missing_block_hashes.iter().map(ToOwned::to_owned))
            .collect::<Vec<_>>();
        packed::GetBlocksProof::new_builder()
            .last_hash(last_header.hash())
            .block_hashes(block_hashes.pack())
            .build()
    }

    fn build_proof_by_numbers(
        &self,
        last_num: BlockNumber,
        block_nums: &[BlockNumber],
    ) -> packed::HeaderDigestVec {
        let positions = block_nums
            .iter()
            .map(|num| leaf_index_to_pos(*num))
            .collect::<Vec<_>>();
        if positions.is_empty() {
            Default::default()
        } else {
            self.shared()
                .snapshot()
                .chain_root_mmr(last_num - 1)
                .gen_proof(positions)
                .expect("generate proof")
                .proof_items()
                .to_owned()
                .pack()
        }
    }
}

impl SnapshotExt for Snapshot {
    fn get_header_by_number(&self, num: BlockNumber) -> Option<HeaderView> {
        self.get_block_hash(num)
            .and_then(|hash| self.get_block_header(&hash))
    }

    fn get_block_by_number(&self, num: BlockNumber) -> Option<BlockView> {
        self.get_block_hash(num)
            .and_then(|hash| self.get_block(&hash))
    }

    fn get_block_ext_by_number(&self, num: BlockNumber) -> Option<BlockExt> {
        self.get_block_hash(num)
            .and_then(|hash| self.get_block_ext(&hash))
    }

    fn get_verifiable_header_by_number(
        &self,
        num: BlockNumber,
    ) -> Option<packed::VerifiableHeader> {
        self.get_block_by_number(num).map(|block| {
            let parent_chain_root = if num == 0 {
                Default::default()
            } else {
                self.chain_root_mmr(num - 1)
                    .get_root()
                    .expect("has chain root")
            };
            packed::VerifiableHeader::new_builder()
                .header(block.data().header())
                .uncles_hash(block.calc_uncles_hash())
                .extension(Pack::pack(&block.extension()))
                .parent_chain_root(parent_chain_root)
                .build()
        })
    }
}
