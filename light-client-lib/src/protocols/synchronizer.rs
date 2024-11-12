use ckb_constant::sync::INIT_BLOCKS_IN_TRANSIT_PER_PEER;
use ckb_network::{
    async_trait, bytes::Bytes, BoxedCKBProtocolContext, CKBProtocolHandler, PeerIndex,
};
use ckb_types::{packed, prelude::*};
use log::{info, trace, warn};
use std::collections::HashSet;
use std::sync::Arc;

use super::BAD_MESSAGE_BAN_TIME;
use crate::protocols::Peers;
use crate::storage::Storage;
use crate::utils::network::prove_or_download_matched_blocks;

pub struct SyncProtocol {
    storage: Storage,
    peers: Arc<Peers>,
}

impl SyncProtocol {
    pub fn new(storage: Storage, peers: Arc<Peers>) -> Self {
        Self { storage, peers }
    }
}

#[async_trait]
impl CKBProtocolHandler for SyncProtocol {
    async fn init(&mut self, _nc: BoxedCKBProtocolContext) {}

    async fn connected(&mut self, _nc: BoxedCKBProtocolContext, peer: PeerIndex, version: &str) {
        info!("SyncProtocol({}).connected peer={}", version, peer);
    }

    async fn disconnected(&mut self, _nc: BoxedCKBProtocolContext, peer: PeerIndex) {
        info!("SyncProtocol.disconnected peer={}", peer);
    }

    async fn received(&mut self, nc: BoxedCKBProtocolContext, peer: PeerIndex, data: Bytes) {
        let message = match packed::SyncMessageReader::from_compatible_slice(&data) {
            Ok(msg) => msg.to_enum(),
            _ => {
                warn!(
                    "SyncProtocol.received a malformed message from Peer({})",
                    peer
                );
                nc.ban_peer(
                    peer,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        trace!(
            "SyncProtocol.received peer={}, message={}",
            peer,
            message.item_name()
        );
        match message {
            packed::SyncMessageUnionReader::SendBlock(reader) => {
                let new_block = reader.to_entity().block();
                let matched_blocks = self.peers.matched_blocks();
                self.peers
                    .add_block(&mut matched_blocks.write().expect("poisoned"), new_block);

                let option = {
                    let matched_blocks = matched_blocks.read().expect("poisoned");
                    !matched_blocks.is_empty()
                        && self.peers.all_matched_blocks_downloaded(&matched_blocks)
                };
                if option {
                    let (start_number, blocks_count, db_blocks) = self
                        .storage
                        .get_earliest_matched_blocks_async()
                        .await
                        .expect("get matched blocks from storage");
                    let db_blocks: HashSet<_> =
                        db_blocks.into_iter().map(|(hash, _)| hash).collect();

                    self.storage.remove_matched_blocks_async(start_number).await;
                    let blocks = self
                        .peers
                        .clear_matched_blocks(&mut matched_blocks.write().expect("poisoned"));
                    assert_eq!(blocks.len(), db_blocks.len());
                    info!(
                        "all matched blocks downloaded, start_number={}, blocks_count={}, matched_count={}",
                        start_number,
                        blocks_count,
                        db_blocks.len()
                    );

                    // update storage
                    for block in blocks {
                        assert!(db_blocks.contains(&block.header().calc_header_hash()));
                        self.storage.filter_block_async(block).await;
                    }
                    self.storage
                        .update_block_number_async(start_number + blocks_count - 1)
                        .await;

                    // send more GetBlocksProof/GetBlocks requests
                    if let Some((_start_number, _blocks_count, db_blocks)) =
                        self.storage.get_earliest_matched_blocks_async().await
                    {
                        self.peers.add_matched_blocks(
                            &mut matched_blocks.write().expect("poisoned"),
                            db_blocks,
                        );
                        let tip_header = self.storage.get_tip_header_async().await;
                        prove_or_download_matched_blocks(
                            Arc::clone(&self.peers),
                            &tip_header,
                            &matched_blocks.read().expect("poisoned"),
                            nc.as_ref(),
                            INIT_BLOCKS_IN_TRANSIT_PER_PEER,
                        );
                    }
                }
            }
            _ => {
                let content = packed::InIBD::new_builder().build();
                let msg = packed::SyncMessage::new_builder().set(content).build();
                if let Err(err) = nc.send_message_to(peer, msg.as_bytes()) {
                    warn!(
                        "SyncProtocol.received failed to send InIBD message to peer={} since {:?}",
                        peer, err
                    );
                }
            }
        }
    }
}
