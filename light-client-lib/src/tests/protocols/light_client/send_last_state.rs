use ckb_network::{CKBProtocolHandler, PeerIndex, SupportProtocols};
use ckb_types::{
    core::{EpochNumberWithFraction, HeaderBuilder},
    packed::{self},
    prelude::*,
    utilities::merkle_mountain_range::VerifiableHeader,
};

use crate::{
    protocols::{LastState, ProveRequest, ProveState, StatusCode},
    tests::{
        prelude::*,
        utils::{MockChain, MockNetworkContext},
    },
};

#[tokio::test]
async fn peer_state_is_not_found() {
    let chain = MockChain::new_with_dummy_pow("test-light-client");
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peers = chain.create_peers();
    let mut protocol = chain.create_light_client_protocol(peers);

    let data = {
        let content = packed::SendLastState::new_builder().build();
        packed::LightClientMessage::new_builder()
            .set(content)
            .build()
    }
    .as_bytes();

    let peer_index = PeerIndex::new(1);
    protocol.received(nc.context(), peer_index, data).await;

    assert!(nc.banned_since(peer_index, StatusCode::PeerIsNotFound));
}

#[tokio::test]
async fn invalid_nonce() {
    let chain = MockChain::new_with_default_pow("test-light-client");
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let data = {
        let content = packed::SendLastState::new_builder().build();
        packed::LightClientMessage::new_builder()
            .set(content)
            .build()
    }
    .as_bytes();

    protocol.received(nc.context(), peer_index, data).await;

    assert!(nc.banned_since(peer_index, StatusCode::InvalidNonce));
}

#[tokio::test]
async fn invalid_chain_root() {
    let chain = MockChain::new_with_dummy_pow("test-light-client");
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let bad_message_allowed_each_hour = 5;
    let peers = {
        let peers = chain.create_peers_with_parameters(bad_message_allowed_each_hour);
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let data = {
        let header = HeaderBuilder::default()
            .epoch(EpochNumberWithFraction::new(1, 1, 10).pack())
            .number(11u64.pack())
            .build();
        let last_header = packed::VerifiableHeader::new_builder()
            .header(header.data())
            .build();
        let content = packed::SendLastState::new_builder()
            .last_header(last_header)
            .build();
        packed::LightClientMessage::new_builder()
            .set(content)
            .build()
    }
    .as_bytes();

    for _ in 0..bad_message_allowed_each_hour {
        protocol
            .received(nc.context(), peer_index, data.clone())
            .await;
        assert!(nc.not_banned(peer_index));
    }

    protocol.received(nc.context(), peer_index, data).await;
    assert!(nc.banned_since(peer_index, StatusCode::InvalidChainRoot));
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_last_state() {
    let chain = MockChain::new_with_dummy_pow("test-light-client").start();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let num = 12;
    chain.mine_to(12);

    let snapshot = chain.shared().snapshot();

    let last_header = snapshot
        .get_verifiable_header_by_number(num)
        .expect("block stored");
    let last_hash = last_header.header().calc_header_hash();
    let data = {
        let content = packed::SendLastState::new_builder()
            .last_header(last_header)
            .build();
        packed::LightClientMessage::new_builder()
            .set(content)
            .build()
    }
    .as_bytes();

    let peer_state = protocol
        .get_peer_state(&peer_index)
        .expect("has peer state");
    assert!(peer_state.get_last_state().is_none());
    assert!(nc.sent_messages().borrow().is_empty());

    protocol.received(nc.context(), peer_index, data).await;

    assert!(nc.not_banned(peer_index));

    let peer_state = protocol
        .get_peer_state(&peer_index)
        .expect("has peer state");
    assert!(peer_state.get_last_state().is_some());
    assert_eq!(nc.sent_messages().borrow().len(), 1);

    let data = &nc.sent_messages().borrow()[0].2;
    let message = packed::LightClientMessageReader::new_unchecked(&data);
    let content = if let packed::LightClientMessageUnionReader::GetLastStateProof(content) =
        message.to_enum()
    {
        content
    } else {
        panic!("unexpected message");
    };
    assert_eq!(content.last_hash().as_slice(), last_hash.as_slice());
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_same_last_state() {
    let chain = MockChain::new_with_dummy_pow("test-light-client").start();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let num = 12;
    chain.mine_to(num);

    let snapshot = chain.shared().snapshot();
    let last_header = snapshot
        .get_verifiable_header_by_number(num)
        .expect("block stored");
    let data = {
        let content = packed::SendLastState::new_builder()
            .last_header(last_header)
            .build();
        packed::LightClientMessage::new_builder()
            .set(content)
            .build()
    }
    .as_bytes();

    // Setup the test fixture:
    // - Update last state.
    {
        protocol
            .received(nc.context(), peer_index, data.clone())
            .await;
    }

    // Run the test.
    {
        let peer_state_before = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state");
        let last_state_before = peer_state_before.get_last_state().expect("has last state");

        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        protocol.received(nc.context(), peer_index, data).await;

        let peer_state_after = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state");
        let last_state_after = peer_state_after.get_last_state().expect("has last state");

        assert!(last_state_after.is_same_as(&last_state_before));
        assert_eq!(last_state_after.update_ts(), last_state_before.update_ts());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_continuous_last_state() {
    let chain = MockChain::new_with_dummy_pow("test-light-client").start();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let mut num = 12;
    chain.mine_to(num + 1);

    let snapshot = chain.shared().snapshot();

    // Setup the test fixture:
    // - Update last state.
    // - Commit prove state.
    {
        let peer_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state");
        assert!(peer_state.get_prove_state().is_none());
        let prove_request = {
            let last_header: VerifiableHeader = snapshot
                .get_verifiable_header_by_number(num)
                .expect("block stored")
                .into();
            let content = protocol
                .build_prove_request_content(&peer_state, &last_header)
                .await
                .expect("build prove request content");
            let last_state = LastState::new(last_header);
            protocol
                .peers()
                .update_last_state(peer_index, last_state.clone())
                .unwrap();
            ProveRequest::new(last_state, content)
        };
        protocol
            .peers()
            .update_prove_request(peer_index, prove_request.clone())
            .unwrap();
        let prove_state = {
            let last_n_headers = (1..num)
                .into_iter()
                .map(|num| snapshot.get_header_by_number(num).expect("block stored"))
                .collect::<Vec<_>>();
            ProveState::new_from_request(prove_request, Vec::new(), last_n_headers)
        };
        protocol
            .commit_prove_state(peer_index, prove_state)
            .await
            .unwrap();
    }

    num += 1;

    // Run the test.
    {
        let last_header = snapshot
            .get_verifiable_header_by_number(num)
            .expect("block stored");
        let data = {
            let content = packed::SendLastState::new_builder()
                .last_header(last_header.clone())
                .build();
            packed::LightClientMessage::new_builder()
                .set(content)
                .build()
        }
        .as_bytes();
        let last_header: VerifiableHeader = last_header.into();
        let last_state = LastState::new(last_header.clone());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(prove_state.is_parent_of(&last_state));

        protocol.received(nc.context(), peer_index, data).await;

        assert!(nc.sent_messages().borrow().is_empty());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(prove_state.is_same_as(&last_header));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_noncontinuous_last_state() {
    let chain = MockChain::new_with_dummy_pow("test-light-client").start();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let mut num = 12;
    chain.mine_to(num + 2);

    let snapshot = chain.shared().snapshot();

    // Setup the test fixture:
    // - Update last state.
    // - Commit prove state.
    {
        let peer_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state");
        assert!(peer_state.get_prove_state().is_none());
        let prove_request = {
            let last_header: VerifiableHeader = snapshot
                .get_verifiable_header_by_number(num)
                .expect("block stored")
                .into();
            let content = protocol
                .build_prove_request_content(&peer_state, &last_header)
                .await
                .expect("build prove request content");
            let last_state = LastState::new(last_header);
            protocol
                .peers()
                .update_last_state(peer_index, last_state.clone())
                .unwrap();
            ProveRequest::new(last_state, content)
        };
        protocol
            .peers()
            .update_prove_request(peer_index, prove_request.clone())
            .unwrap();
        let prove_state = {
            let last_n_headers = (1..num)
                .into_iter()
                .map(|num| snapshot.get_header_by_number(num).expect("block stored"))
                .collect::<Vec<_>>();
            ProveState::new_from_request(prove_request, Vec::new(), last_n_headers)
        };
        protocol
            .commit_prove_state(peer_index, prove_state)
            .await
            .unwrap();
    }

    num += 2;

    // Run the test.
    {
        let last_header = snapshot
            .get_verifiable_header_by_number(num)
            .expect("block stored");
        let data = {
            let content = packed::SendLastState::new_builder()
                .last_header(last_header.clone())
                .build();
            packed::LightClientMessage::new_builder()
                .set(content)
                .build()
        }
        .as_bytes();
        let last_header: VerifiableHeader = last_header.into();
        let last_state = LastState::new(last_header.clone());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(!prove_state.is_parent_of(&last_state));

        protocol.received(nc.context(), peer_index, data).await;

        assert!(nc.sent_messages().borrow().is_empty());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(!prove_state.is_same_as(&last_header));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_continuous_but_forked_last_state() {
    let chain = MockChain::new_with_dummy_pow("test-light-client").start();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.request_last_state(peer_index).unwrap();
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let mut num = 12;
    chain.mine_to_with(num + 1, |block| {
        let block_number: u64 = block.header().raw().number().unpack();
        block
            .as_advanced_builder()
            .timestamp((100 + block_number).pack())
            .build()
    });

    // Setup the test fixture:
    // - Update last state.
    // - Commit prove state.
    {
        let snapshot = chain.shared().snapshot();
        let peer_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state");
        assert!(peer_state.get_prove_state().is_none());
        let prove_request = {
            let last_header: VerifiableHeader = snapshot
                .get_verifiable_header_by_number(num)
                .expect("block stored")
                .into();
            let content = protocol
                .build_prove_request_content(&peer_state, &last_header)
                .await
                .expect("build prove request content");
            let last_state = LastState::new(last_header);
            protocol
                .peers()
                .update_last_state(peer_index, last_state.clone())
                .unwrap();
            ProveRequest::new(last_state, content)
        };
        protocol
            .peers()
            .update_prove_request(peer_index, prove_request.clone())
            .unwrap();
        let prove_state = {
            let last_n_headers = (1..num)
                .into_iter()
                .map(|num| snapshot.get_header_by_number(num).expect("block stored"))
                .collect::<Vec<_>>();
            ProveState::new_from_request(prove_request, Vec::new(), last_n_headers)
        };
        protocol
            .commit_prove_state(peer_index, prove_state)
            .await
            .unwrap();
    }

    let prev_last_header: VerifiableHeader = chain
        .shared()
        .snapshot()
        .get_verifiable_header_by_number(num)
        .expect("block stored")
        .into();
    {
        chain.rollback_to(num - 5, Default::default());
        num += 1;
        chain.mine_to_with(num, |block| {
            let block_number: u64 = block.header().raw().number().unpack();
            block
                .as_advanced_builder()
                .timestamp((200 + block_number).pack())
                .build()
        });
        assert_eq!(chain.shared().snapshot().tip_number(), num);
    }

    // Run the test.
    {
        let last_header = chain
            .shared()
            .snapshot()
            .get_verifiable_header_by_number(num)
            .expect("block stored");
        let data = {
            let content = packed::SendLastState::new_builder()
                .last_header(last_header.clone())
                .build();
            packed::LightClientMessage::new_builder()
                .set(content)
                .build()
        }
        .as_bytes();
        let last_header: VerifiableHeader = last_header.into();
        let last_state = LastState::new(last_header.clone());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(!prove_state.is_parent_of(&last_state));

        protocol.received(nc.context(), peer_index, data).await;

        assert!(nc.sent_messages().borrow().is_empty());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(!prove_state.is_same_as(&last_header));

        assert!(prove_state.is_same_as(&prev_last_header));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_proved_last_state() {
    let chain = MockChain::new_with_dummy_pow("test-light-client").start();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    let peer_index = PeerIndex::new(1);
    let peer_index_proved = PeerIndex::new(2);
    let peers = {
        let peers = chain.create_peers();
        peers.add_peer(peer_index);
        peers.add_peer(peer_index_proved);
        peers
    };
    let mut protocol = chain.create_light_client_protocol(peers);

    let num = 12;
    chain.mine_to(num + 2);

    let snapshot = chain.shared().snapshot();

    // Setup the test fixture.
    {
        let peer_state = protocol
            .get_peer_state(&peer_index_proved)
            .expect("has peer state");
        let last_header: VerifiableHeader = snapshot
            .get_verifiable_header_by_number(num)
            .expect("block stored")
            .into();
        let prove_request = {
            let content = protocol
                .build_prove_request_content(&peer_state, &last_header)
                .await
                .expect("build prove request content");
            let last_state = LastState::new(last_header.clone());
            ProveRequest::new(last_state, content)
        };
        let prove_state = {
            let last_n_headers = (1..num)
                .into_iter()
                .map(|num| snapshot.get_header_by_number(num).expect("block stored"))
                .collect::<Vec<_>>();
            ProveState::new_from_request(prove_request.clone(), Vec::new(), last_n_headers)
        };
        protocol
            .peers()
            .mock_prove_request(peer_index_proved, prove_request)
            .unwrap();
        protocol
            .commit_prove_state(peer_index_proved, prove_state)
            .await
            .unwrap();

        let prove_state = protocol
            .get_peer_state(&peer_index_proved)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(prove_state.is_same_as(&last_header));
    }

    // Run the test.
    {
        let last_header = snapshot
            .get_verifiable_header_by_number(num)
            .expect("block stored");
        let data = {
            let content = packed::SendLastState::new_builder()
                .last_header(last_header.clone())
                .build();
            packed::LightClientMessage::new_builder()
                .set(content)
                .build()
        }
        .as_bytes();
        let last_header: VerifiableHeader = last_header.into();

        protocol.peers().request_last_state(peer_index).unwrap();
        protocol.received(nc.context(), peer_index, data).await;

        assert!(nc.sent_messages().borrow().is_empty());

        let prove_state = protocol
            .get_peer_state(&peer_index)
            .expect("has peer state")
            .get_prove_state()
            .expect("has prove state")
            .to_owned();
        assert!(prove_state.is_same_as(&last_header));
    }
}
