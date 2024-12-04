use ckb_network::{BoxedCKBProtocolContext, PeerIndex, SupportProtocols};
use ckb_types::{
    core::{ExtraHashView, HeaderView},
    packed,
    prelude::*,
    utilities::merkle_mountain_range::VerifiableHeader,
};
use log::{debug, error, info};
use rand::seq::SliceRandom;

use crate::storage::HeaderWithExtension;

use super::{
    super::{LightClientProtocol, Status, StatusCode},
    verify_mmr_proof,
};

pub(crate) struct SendBlocksProofProcess<'a> {
    message: packed::SendBlocksProofReader<'a>,
    protocol: &'a mut LightClientProtocol,
    peer_index: PeerIndex,
    nc: &'a BoxedCKBProtocolContext,
}

impl<'a> SendBlocksProofProcess<'a> {
    pub(crate) fn new(
        message: packed::SendBlocksProofReader<'a>,
        protocol: &'a mut LightClientProtocol,
        peer_index: PeerIndex,
        nc: &'a BoxedCKBProtocolContext,
    ) -> Self {
        Self {
            message,
            protocol,
            peer_index,
            nc,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        let status = self.execute_internally().await;
        self.protocol
            .peers()
            .update_blocks_proof_request(self.peer_index, None, false);
        status
    }

    async fn execute_internally(&self) -> Status {
        let peer = return_if_failed!(self.protocol.get_peer(&self.peer_index));

        let original_request = if let Some(original_request) = peer.get_blocks_proof_request() {
            original_request
        } else {
            error!("peer {} isn't waiting for a proof", self.peer_index);
            return StatusCode::PeerIsNotOnProcess.into();
        };

        let last_header: VerifiableHeader = self.message.last_header().to_entity().into();

        // Update the last state if the response contains a new one.
        if original_request.last_hash() != last_header.header().hash() {
            if self.message.proof().is_empty()
                && self.message.headers().is_empty()
                && self.message.missing_block_hashes().is_empty()
            {
                return_if_failed!(self
                    .protocol
                    .process_last_state(self.peer_index, last_header));
                self.protocol
                    .peers()
                    .mark_fetching_headers_timeout(self.peer_index);
                return Status::ok();
            } else {
                // Since the last state is different, then no data should be contained.
                error!(
                    "peer {} send a proof with different last state",
                    self.peer_index
                );
                return StatusCode::UnexpectedResponse.into();
            }
        }

        let headers: Vec<_> = self
            .message
            .headers()
            .iter()
            .map(|header| header.to_entity().into_view())
            .collect();

        // Check if the response is match the request.
        let received_block_hashes = headers
            .iter()
            .map(|header| header.hash())
            .collect::<Vec<_>>();
        let missing_block_hashes = self
            .message
            .missing_block_hashes()
            .to_entity()
            .into_iter()
            .collect::<Vec<_>>();
        if !original_request.check_block_hashes(&received_block_hashes, &missing_block_hashes) {
            error!("peer {} send an unknown proof", self.peer_index);
            return StatusCode::UnexpectedResponse.into();
        }

        // If all blocks are missing.
        if self.message.headers().is_empty() {
            if !self.message.proof().is_empty() {
                error!(
                    "peer {} send a proof when all blocks are missing",
                    self.peer_index
                );
                return StatusCode::UnexpectedResponse.into();
            }
        } else {
            // Check PoW for blocks
            return_if_failed!(self.protocol.check_pow_for_headers(headers.iter()));

            // Check extra hash for blocks
            let is_v1 = self.message.count_extra_fields() >= 2;
            let extensions = if is_v1 {
                let message_v1 =
                    packed::SendBlocksProofV1Reader::new_unchecked(self.message.as_slice());
                let uncle_hashes: Vec<_> = message_v1
                    .blocks_uncles_hash()
                    .iter()
                    .map(|uncle_hashes| uncle_hashes.to_entity())
                    .collect();

                let extensions: Vec<_> = message_v1
                    .blocks_extension()
                    .iter()
                    .map(|extension| extension.to_entity().to_opt())
                    .collect();

                return_if_failed!(verify_extra_hash(&headers, &uncle_hashes, &extensions));
                extensions
            } else {
                vec![None; headers.len()]
            };

            // Verify the proof
            return_if_failed!(verify_mmr_proof(
                self.protocol.mmr_activated_epoch(),
                &last_header,
                self.message.proof(),
                headers.iter(),
            ));

            // Get blocks
            if original_request.should_get_blocks() {
                let block_hashes: Vec<packed::Byte32> =
                    headers.iter().map(|header| header.hash()).collect();
                {
                    #[cfg(target_arch = "wasm32")]
                    let mut matched_blocks = self.protocol.peers().matched_blocks().write().await;
                    #[cfg(not(target_arch = "wasm32"))]
                    let mut matched_blocks = self
                        .protocol
                        .peers()
                        .matched_blocks()
                        .write()
                        .expect("poisoned");
                    self.protocol
                        .peers
                        .mark_matched_blocks_proved(&mut matched_blocks, &block_hashes);
                }

                let best_peers: Vec<_> = self
                    .protocol
                    .peers
                    .get_best_proved_peers(&last_header.header().data())
                    .into_iter()
                    .filter_map(|peer_index| {
                        self.protocol
                            .peers
                            .get_peer(&peer_index)
                            .map(|peer| (peer_index, peer))
                    })
                    .collect();

                if let Some((peer_index, _)) = best_peers
                    .iter()
                    .filter(|(_peer_index, peer)| peer.get_blocks_request().is_none())
                    .collect::<Vec<_>>()
                    .choose(&mut rand::thread_rng())
                {
                    self.protocol
                        .peers
                        .update_blocks_request(*peer_index, Some(block_hashes.clone()));
                    debug!(
                        "send get blocks request to peer: {}, matched_count: {}",
                        peer_index,
                        block_hashes.len()
                    );
                    for hashes in
                        block_hashes.chunks(self.protocol.init_blocks_in_transit_per_peer())
                    {
                        let content = packed::GetBlocks::new_builder()
                            .block_hashes(hashes.to_vec().pack())
                            .build();
                        let message = packed::SyncMessage::new_builder()
                            .set(content)
                            .build()
                            .as_bytes();
                        if let Err(err) = self.nc.send_message(
                            SupportProtocols::Sync.protocol_id(),
                            *peer_index,
                            message,
                        ) {
                            let error_message =
                                format!("nc.send_message SyncMessage, error: {:?}", err);
                            info!("{}", error_message);
                            return StatusCode::Network.with_context(error_message);
                        }
                    }
                }
            }

            for (header, extension) in headers.into_iter().zip(extensions.into_iter()) {
                if self.protocol.peers().remove_fetching_header(&header.hash()) {
                    self.protocol
                        .storage()
                        .add_fetched_header(&HeaderWithExtension {
                            header: header.data(),
                            extension,
                        });
                }
            }
        }
        self.protocol
            .peers()
            .mark_fetching_headers_missing(&missing_block_hashes);
        Status::ok()
    }
}

pub(crate) fn verify_extra_hash(
    headers: &[HeaderView],
    uncle_hashes: &[packed::Byte32],
    extensions: &[Option<packed::Bytes>],
) -> Result<(), Status> {
    if headers.len() != uncle_hashes.len() || headers.len() != extensions.len() {
        return Err(StatusCode::InvalidProof.into());
    }

    for ((header, uncle_hash), extension) in headers
        .iter()
        .zip(uncle_hashes.iter())
        .zip(extensions.iter())
    {
        let expected_extension_hash = extension
            .as_ref()
            .map(|extension| extension.calc_raw_data_hash());
        let extra_hash_view = ExtraHashView::new(uncle_hash.clone(), expected_extension_hash);
        let expected_extra_hash = extra_hash_view.extra_hash();
        let actual_extra_hash = header.extra_hash();
        if expected_extra_hash != actual_extra_hash {
            return Err(StatusCode::InvalidProof.into());
        }
    }

    Ok(())
}
