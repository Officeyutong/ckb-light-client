mod utils;

use std::{
    ops::Bound,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, RwLock,
    },
};

use ckb_light_client_lib::{
    error::Error,
    protocols::{
        FilterProtocol, LightClientProtocol, Peers, PendingTxs, RelayProtocol, SyncProtocol,
        BAD_MESSAGE_ALLOWED_EACH_HOUR, CHECK_POINT_INTERVAL,
    },
    service::{Cell, Order, Pagination, ScriptStatus, ScriptType, SearchKey, SetScriptsCommand},
    storage::{extract_raw_data, Direction, Key, KeyPrefix, Storage, StorageWithChainData},
    types::RunEnv,
};
use wasm_bindgen::prelude::*;

use ckb_chain_spec::ChainSpec;
use ckb_jsonrpc_types::{Deserialize, JsonBytes, Serialize};
use ckb_network::{
    CKBProtocol, CKBProtocolHandler, Flags, NetworkController, NetworkService, NetworkState,
    SupportProtocols,
};
use ckb_resource::Resource;
use ckb_stop_handler::broadcast_exit_signals;
use ckb_types::{core, packed, prelude::*};

use std::sync::OnceLock;

static MAINNET_CONFIG: &'static str = include_str!("../../config/mainnet.toml");

static TESTNET_CONFIG: &'static str = include_str!("../../config/testnet.toml");

static DEV_CONFIG: &'static str = include_str!("../../config/dev.toml");

static DEV_SOURCE: &'static str = include_str!("../../../ckb/node3/specs/dev.toml");

static STORAGE_WITH_DATA: OnceLock<StorageWithChainData> = OnceLock::new();

static NET_CONTROL: OnceLock<NetworkController> = OnceLock::new();

/// 0b0 init
/// 0b1 start
/// 0b10 stop
static START_FLAG: AtomicU8 = AtomicU8::new(0);

fn status(flag: u8) -> bool {
    START_FLAG.load(Ordering::SeqCst) & flag == flag
}

fn change_status(flag: u8) {
    START_FLAG.store(flag, Ordering::SeqCst);
}

#[wasm_bindgen]
pub async fn ligth_client(net_flag: String) -> Result<(), JsValue> {
    if !status(0b0) {
        return Err(JsValue::from_str("Can't start twice"));
    }
    utils::set_panic_hook();
    wasm_logger::init(wasm_logger::Config::new(log::Level::Info));

    let config = match net_flag.as_str() {
        "testnet" => TESTNET_CONFIG.parse::<RunEnv>().unwrap(),
        "mainnet" => MAINNET_CONFIG.parse::<RunEnv>().unwrap(),
        "dev" => DEV_CONFIG.parse::<RunEnv>().unwrap(),
        _ => panic!("unsupport flag"),
    };

    let storage = Storage::new(&config.store.path).await;
    let chain_spec = ChainSpec::load_from(&match config.chain.as_str() {
        "mainnet" => Resource::bundled("specs/mainnet.toml".to_string()),
        "testnet" => Resource::bundled("specs/testnet.toml".to_string()),
        "dev" => Resource::raw(DEV_SOURCE.to_string()),
        _ => panic!("unsupport flag"),
    })
    .expect("load spec should be OK");

    let consensus = chain_spec
        .build_consensus()
        .expect("build consensus should be OK");
    let genesis = consensus.genesis_block().data();

    storage.init_genesis_block(genesis).await;
    log::info!("4");

    let pending_txs = Arc::new(RwLock::new(PendingTxs::default()));
    let max_outbound_peers = config.network.max_outbound_peers;
    let network_state = NetworkState::from_config(config.network)
        .map(|network_state| {
            Arc::new(network_state.required_flags(
                Flags::DISCOVERY
                    | Flags::SYNC
                    | Flags::RELAY
                    | Flags::LIGHT_CLIENT
                    | Flags::BLOCK_FILTER,
            ))
        })
        .map_err(|err| {
            let errmsg = format!("failed to initialize network state since {}", err);
            Error::runtime(errmsg)
        })
        .unwrap();
    let required_protocol_ids = vec![
        SupportProtocols::Sync.protocol_id(),
        SupportProtocols::LightClient.protocol_id(),
        SupportProtocols::Filter.protocol_id(),
    ];
    log::info!("3");
    let peers = Arc::new(Peers::new(
        max_outbound_peers,
        CHECK_POINT_INTERVAL,
        storage.get_last_check_point_async().await,
        BAD_MESSAGE_ALLOWED_EACH_HOUR,
    ));

    log::info!("7");

    let sync_protocol = SyncProtocol::new(storage.clone(), Arc::clone(&peers));
    let relay_protocol_v2 = RelayProtocol::new(
        pending_txs.clone(),
        Arc::clone(&peers),
        consensus.clone(),
        storage.clone(),
        false,
    );
    let relay_protocol_v3 = RelayProtocol::new(
        pending_txs.clone(),
        Arc::clone(&peers),
        consensus.clone(),
        storage.clone(),
        true,
    );
    let light_client: Box<dyn CKBProtocolHandler> = Box::new(LightClientProtocol::new(
        storage.clone(),
        Arc::clone(&peers),
        consensus.clone(),
    ));

    let filter_protocol = FilterProtocol::new(storage.clone(), Arc::clone(&peers));

    let protocols = vec![
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Sync,
            Box::new(sync_protocol),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::RelayV2,
            Box::new(relay_protocol_v2),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::RelayV3,
            Box::new(relay_protocol_v3),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::LightClient,
            light_client,
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Filter,
            Box::new(filter_protocol),
            Arc::clone(&network_state),
        ),
    ];

    let handle = ckb_async_runtime::Handle {};
    let network_controller = NetworkService::new(
        Arc::clone(&network_state),
        protocols,
        required_protocol_ids,
        (
            consensus.identify_name(),
            "0.1.0".to_owned(),
            Flags::DISCOVERY,
        ),
    )
    .start(&handle)
    .map_err(|err| {
        let errmsg = format!("failed to start network since {}", err);
        Error::runtime(errmsg)
    })
    .unwrap();

    let storage_with_data = StorageWithChainData::new(storage, peers, pending_txs);

    STORAGE_WITH_DATA.get_or_init(|| storage_with_data);
    NET_CONTROL.get_or_init(|| network_controller);
    change_status(0b1);
    Ok(())
}

#[wasm_bindgen]
pub fn stop() {
    broadcast_exit_signals();
    change_status(0b10);
}

use ckb_types::prelude::IntoHeaderView;

#[wasm_bindgen]
pub async fn get_tip_header() -> Result<JsValue, JsValue> {
    Ok(serde_wasm_bindgen::to_value(&Into::<
        ckb_jsonrpc_types::HeaderView,
    >::into(
        STORAGE_WITH_DATA
            .get()
            .unwrap()
            .storage()
            .get_tip_header_async()
            .await
            .into_view(),
    ))?)
}

// #[wasm_bindgen]
// pub fn get_genesis_block() -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn get_header(hash: &[u8]) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn fetch_header(hash: &[u8]) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn estimate_cycles(tx: JsValue) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn local_node_info() -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn get_peers() -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

#[wasm_bindgen]
pub fn get_script_example() -> JsValue {
    let script = serde_json::from_str::<ckb_jsonrpc_types::Script>(
        r#"
    {
       "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
       "hash_type": "type",
       "args": "0x418ac4485a3dbe8221321b249631cf8491b31fd1"
     }
     "#,
    )
    .unwrap();

    serde_wasm_bindgen::to_value(&ScriptStatus {
        script,
        script_type: ScriptType::Lock,
        block_number: 0.into(),
    })
    .unwrap()
}

#[wasm_bindgen]
pub fn get_search_key_example() -> JsValue {
    let script = serde_json::from_str::<ckb_jsonrpc_types::Script>(
        r#"
    {
       "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
       "hash_type": "type",
       "args": "0x418ac4485a3dbe8221321b249631cf8491b31fd1"
     }
     "#,
    )
    .unwrap();

    let key = SearchKey {
        script,
        script_type: ScriptType::Lock,
        filter: None,
        with_data: None,
        group_by_transaction: None,
    };

    serde_wasm_bindgen::to_value(&key).unwrap()
}

#[wasm_bindgen]
pub async fn set_scripts(
    scripts: Vec<JsValue>,
    command: Option<SetScriptsCommand>,
) -> Result<(), JsValue> {
    let scripts: Vec<ScriptStatus> = scripts
        .into_iter()
        .map(|v| serde_wasm_bindgen::from_value::<ScriptStatus>(v))
        .collect::<Result<Vec<_>, _>>()?;

    STORAGE_WITH_DATA
        .get()
        .unwrap()
        .storage()
        .update_filter_scripts(
            scripts.into_iter().map(Into::into).collect(),
            command.map(Into::into).unwrap_or_default(),
        )
        .await;

    Ok(())
}

#[wasm_bindgen]
pub async fn get_scripts() -> Result<Vec<JsValue>, JsValue> {
    if !status(0b1) {
        return Err(JsValue::from_str("light client not on start state"));
    }
    let scripts = STORAGE_WITH_DATA
        .get()
        .unwrap()
        .storage()
        .get_filter_scripts_async()
        .await;

    Ok(scripts
        .into_iter()
        .map(Into::into)
        .map(|v: ScriptStatus| serde_wasm_bindgen::to_value(&v))
        .collect::<Result<Vec<_>, _>>()?)
}

#[wasm_bindgen]
pub async fn get_cells(
    search_key: JsValue,
    order: Order,
    limit: u32,
    after_cursor: Option<Vec<u8>>,
) -> Result<JsValue, JsValue> {
    if !status(0b1) {
        return Err(JsValue::from_str("light client not on start state"));
    }
    let search_key: SearchKey = serde_wasm_bindgen::from_value(search_key)?;

    let (prefix, from_key, direction, skip) = build_query_options(
        &search_key,
        KeyPrefix::CellLockScript,
        KeyPrefix::CellTypeScript,
        order,
        after_cursor.map(JsonBytes::from_vec),
    )?;

    let limit = limit as usize;
    if limit == 0 {
        return Err(JsValue::from_str("limit should be greater than 0"));
    }
    let with_data = search_key.with_data.unwrap_or(true);
    let filter_script_type = match search_key.script_type {
        ScriptType::Lock => ScriptType::Type,
        ScriptType::Type => ScriptType::Lock,
    };

    let (
        filter_prefix,
        filter_script_len_range,
        filter_output_data_len_range,
        filter_output_capacity_range,
        filter_block_range,
    ) = build_filter_options(search_key)?;

    let storage = STORAGE_WITH_DATA.get().unwrap().storage();
    let snapshot = storage.snapshot();

    let iter: Box<dyn Iterator<Item = (&Vec<u8>, &Vec<u8>)>> = match direction {
        Direction::Forward => Box::new(
            snapshot
                .range((Bound::Included(from_key.clone()), Bound::Unbounded))
                .take_while(|(key, _value)| key.starts_with(&prefix)),
        ),
        Direction::Reverse => Box::new(
            snapshot
                .range((Bound::Included(from_key.clone()), Bound::Unbounded))
                .rev()
                .take_while(|(key, _value)| key.starts_with(&prefix)),
        ),
    };

    let mut cells = Vec::new();
    let mut last_key = Vec::new();
    for (key, value) in iter.skip(skip) {
        let tx_hash = packed::Byte32::from_slice(&value).expect("stored tx hash");
        let output_index = u32::from_be_bytes(
            key[key.len() - 4..]
                .try_into()
                .expect("stored output_index"),
        );
        let tx_index = u32::from_be_bytes(
            key[key.len() - 8..key.len() - 4]
                .try_into()
                .expect("stored tx_index"),
        );
        let block_number = u64::from_be_bytes(
            key[key.len() - 16..key.len() - 8]
                .try_into()
                .expect("stored block_number"),
        );

        let tx = packed::Transaction::from_slice(
            &storage
                .get(&Key::TxHash(&tx_hash).into_vec())
                .unwrap()
                .expect("stored tx")[12..],
        )
        .expect("from stored tx slice should be OK");
        let output = tx
            .raw()
            .outputs()
            .get(output_index as usize)
            .expect("get output by index should be OK");
        let output_data = tx
            .raw()
            .outputs_data()
            .get(output_index as usize)
            .expect("get output data by index should be OK");

        if let Some(prefix) = filter_prefix.as_ref() {
            match filter_script_type {
                ScriptType::Lock => {
                    if !extract_raw_data(&output.lock())
                        .as_slice()
                        .starts_with(prefix)
                    {
                        continue;
                    }
                }
                ScriptType::Type => {
                    if output.type_().is_none()
                        || !extract_raw_data(&output.type_().to_opt().unwrap())
                            .as_slice()
                            .starts_with(prefix)
                    {
                        continue;
                    }
                }
            }
        }

        if let Some([r0, r1]) = filter_script_len_range {
            match filter_script_type {
                ScriptType::Lock => {
                    let script_len = extract_raw_data(&output.lock()).len();
                    if script_len < r0 || script_len > r1 {
                        continue;
                    }
                }
                ScriptType::Type => {
                    let script_len = output
                        .type_()
                        .to_opt()
                        .map(|script| extract_raw_data(&script).len())
                        .unwrap_or_default();
                    if script_len < r0 || script_len > r1 {
                        continue;
                    }
                }
            }
        }

        if let Some([r0, r1]) = filter_output_data_len_range {
            if output_data.len() < r0 || output_data.len() >= r1 {
                continue;
            }
        }

        if let Some([r0, r1]) = filter_output_capacity_range {
            let capacity: core::Capacity = output.capacity().unpack();
            if capacity < r0 || capacity >= r1 {
                continue;
            }
        }

        if let Some([r0, r1]) = filter_block_range {
            if block_number < r0 || block_number >= r1 {
                continue;
            }
        }

        if cells.len() >= limit {
            break;
        }

        last_key = key.to_vec();

        cells.push(Cell {
            output: output.into(),
            output_data: if with_data {
                Some(output_data.into())
            } else {
                None
            },
            out_point: packed::OutPoint::new(tx_hash, output_index).into(),
            block_number: block_number.into(),
            tx_index: tx_index.into(),
        });
    }

    Ok(serde_wasm_bindgen::to_value(&Pagination {
        objects: cells,
        last_cursor: JsonBytes::from_vec(last_key),
    })?)
}

// #[wasm_bindgen]
// pub fn get_transactions(
//     search_key: JsValue,
//     order: Order,
//     limit: u32,
//     after_cursor: Option<Vec<u8>>,
// ) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn get_cells_capacity(search_key: JsValue) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn send_transaction(tx: JsValue) -> Result<Vec<u8>, JsValue> {
//     Ok(Vec::new())
// }

// #[wasm_bindgen]
// pub fn get_transaction(hash: &[u8]) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

// #[wasm_bindgen]
// pub fn fetch_transaction(hash: &[u8]) -> Result<JsValue, JsValue> {
//     Ok(JsValue::null())
// }

const MAX_PREFIX_SEARCH_SIZE: usize = u16::max_value() as usize;

// a helper fn to build query options from search paramters, returns prefix, from_key, direction and skip offset
pub fn build_query_options(
    search_key: &SearchKey,
    lock_prefix: KeyPrefix,
    type_prefix: KeyPrefix,
    order: Order,
    after_cursor: Option<JsonBytes>,
) -> Result<(Vec<u8>, Vec<u8>, Direction, usize), JsValue> {
    let mut prefix = match search_key.script_type {
        ScriptType::Lock => vec![lock_prefix as u8],
        ScriptType::Type => vec![type_prefix as u8],
    };
    let script: packed::Script = search_key.script.clone().into();
    let args_len = script.args().len();
    if args_len > MAX_PREFIX_SEARCH_SIZE {
        return Err(JsValue::from_str(&format!(
            "search_key.script.args len should be less than {}",
            MAX_PREFIX_SEARCH_SIZE
        )));
    }
    prefix.extend_from_slice(extract_raw_data(&script).as_slice());

    let (from_key, direction, skip) = match order {
        Order::Asc => after_cursor.map_or_else(
            || (prefix.clone(), Direction::Forward, 0),
            |json_bytes| (json_bytes.as_bytes().into(), Direction::Forward, 1),
        ),
        Order::Desc => after_cursor.map_or_else(
            || {
                (
                    [
                        prefix.clone(),
                        vec![0xff; MAX_PREFIX_SEARCH_SIZE - args_len],
                    ]
                    .concat(),
                    Direction::Reverse,
                    0,
                )
            },
            |json_bytes| (json_bytes.as_bytes().into(), Direction::Reverse, 1),
        ),
    };

    Ok((prefix, from_key, direction, skip))
}

#[allow(clippy::type_complexity)]
pub fn build_filter_options(
    search_key: SearchKey,
) -> Result<
    (
        Option<Vec<u8>>,
        Option<[usize; 2]>,
        Option<[usize; 2]>,
        Option<[core::Capacity; 2]>,
        Option<[core::BlockNumber; 2]>,
    ),
    JsValue,
> {
    let filter = search_key.filter.unwrap_or_default();
    let filter_script_prefix = if let Some(script) = filter.script {
        let script: packed::Script = script.into();
        if script.args().len() > MAX_PREFIX_SEARCH_SIZE {
            return Err(JsValue::from_str(&format!(
                "search_key.filter.script.args len should be less than {}",
                MAX_PREFIX_SEARCH_SIZE
            )));
        }
        let mut prefix = Vec::new();
        prefix.extend_from_slice(extract_raw_data(&script).as_slice());
        Some(prefix)
    } else {
        None
    };

    let filter_script_len_range = filter.script_len_range.map(|[r0, r1]| {
        [
            Into::<u64>::into(r0) as usize,
            Into::<u64>::into(r1) as usize,
        ]
    });

    let filter_output_data_len_range = filter.output_data_len_range.map(|[r0, r1]| {
        [
            Into::<u64>::into(r0) as usize,
            Into::<u64>::into(r1) as usize,
        ]
    });
    let filter_output_capacity_range = filter.output_capacity_range.map(|[r0, r1]| {
        [
            core::Capacity::shannons(r0.into()),
            core::Capacity::shannons(r1.into()),
        ]
    });
    let filter_block_range = filter.block_range.map(|r| [r[0].into(), r[1].into()]);

    Ok((
        filter_script_prefix,
        filter_script_len_range,
        filter_output_data_len_range,
        filter_output_capacity_range,
        filter_block_range,
    ))
}

#[derive(Deserialize, Serialize, Debug)]
struct KV {
    key: Vec<u8>,
    value: Vec<u8>,
}

#[wasm_bindgen]
pub fn test_serde() -> JsValue {
    serde_wasm_bindgen::to_value(&vec![
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 1, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 35, 13, 4, 133, 109, 29, 198, 7, 44, 187, 194, 92, 245, 171, 181, 45, 84,
        19, 215, 216, 155, 25, 199, 70, 207, 158, 191, 19, 246, 151, 180, 1, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 67,
        69, 115, 55, 53, 161, 46, 0, 0, 193, 111, 242, 134, 35, 0, 186, 115, 227, 224, 148, 5, 0,
        0, 0, 153, 245, 75, 1, 251, 254, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ])
    .unwrap()
}

#[wasm_bindgen]
pub async fn test_idb() {
    use idb::{
        DatabaseEvent, Factory, IndexParams, KeyPath, KeyRange, ObjectStoreParams, Request,
        TransactionMode, TransactionResult,
    };

    utils::set_panic_hook();
    wasm_logger::init(wasm_logger::Config::new(log::Level::Info));

    let factory = Factory::new().unwrap();
    // factory.delete("test").unwrap().await.unwrap();

    let mut open_request = factory.open("ckb-light-client", Some(1)).unwrap();
    open_request.on_upgrade_needed(|event| {
        let database = event.database().unwrap();
        let store_params = ObjectStoreParams::new();
        // store_params.auto_increment(true);
        // store_params.key_path(Some(KeyPath::new_single("id")));

        let store = database
            .create_object_store("data/store", store_params)
            .unwrap();
        let mut index_params = IndexParams::new();
        index_params.unique(true);
        store
            .create_index("key", KeyPath::new_single("key"), Some(index_params))
            .unwrap();
    });

    let db = Arc::new(open_request.await.unwrap());

    log::info!("{:?}", db);

    let db_clone = db.clone();

    // let count = index.count(None).unwrap().await;
    // assert_eq!(count, Ok(4), "count should be 4: {count:?}");
    log::info!("1");

    let (tx, rx) = futures::channel::oneshot::channel();
    wasm_bindgen_futures::spawn_local(async move {
        let transaction = db_clone
            .transaction(&["data/store"], TransactionMode::ReadWrite)
            .unwrap();
        let store = transaction.object_store("data/store").unwrap();
        store
            .put(
                &serde_wasm_bindgen::to_value(&KV {
                    key: vec![224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69],
                    value: vec![
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        133, 109, 29, 198, 7, 44, 187, 194, 92, 245, 171, 181, 45, 84, 19, 215,
                        216, 155, 25, 199, 70, 207, 158, 191, 19, 246, 151, 180, 1, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 67, 69, 115, 55, 53, 161, 46, 0, 0, 193,
                        111, 242, 134, 35, 0, 186, 115, 227, 224, 148, 5, 0, 0, 0, 153, 245, 75, 1,
                        251, 254, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    ],
                })
                .unwrap(),
                Some(
                    &serde_wasm_bindgen::to_value(&vec![
                        224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69,
                    ])
                    .unwrap(),
                ),
            )
            .unwrap()
            .await
            .unwrap();
        assert_eq!(
            idb::TransactionResult::Committed,
            transaction.commit().unwrap().await.unwrap()
        );
        tx.send(()).unwrap();
    });

    rx.await.unwrap();
    let (tx, rx) = futures::channel::oneshot::channel();
    let db_clone = db.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let transaction = db_clone
            .transaction(&["data/store"], TransactionMode::ReadWrite)
            .unwrap();
        let store = transaction.object_store("data/store").unwrap();
        store
            .put(
                &serde_wasm_bindgen::to_value(&KV {
                    key: vec![224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69],
                    value: vec![
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 35, 13, 4,
                        133, 109, 29, 198, 7, 44, 187, 194, 92, 245, 171, 181, 45, 84, 19, 215,
                        216, 155, 25, 199, 70, 207, 158, 191, 19, 246, 151, 180, 1, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 67, 69, 115, 55, 53, 161, 46, 0, 0, 193,
                        111, 242, 134, 35, 0, 186, 115, 227, 224, 148, 5, 0, 0, 0, 153, 245, 75, 1,
                        251, 254, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
                    ],
                })
                .unwrap(),
                Some(
                    &serde_wasm_bindgen::to_value(&vec![
                        224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69,
                    ])
                    .unwrap(),
                ),
            )
            .unwrap()
            .await
            .unwrap();
        assert_eq!(
            idb::TransactionResult::Committed,
            transaction.commit().unwrap().await.unwrap()
        );
        tx.send(()).unwrap()
    });

    rx.await.unwrap();

    let transaction = db
        .transaction(&["data/store"], TransactionMode::ReadWrite)
        .unwrap();
    let store = transaction.object_store("data/store").unwrap();

    let index = store.index("key").unwrap();

    let stored_0 = index
        .get(
            serde_wasm_bindgen::to_value(&vec![224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69])
                .unwrap(),
        )
        .unwrap()
        .await;
    log::info!("{:?}", stored_0);
    let start_key = Key::CheckPointIndex(0).into_vec();
    //  [224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69], [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 35, 13, 4, 133, 109, 29, 198, 7, 44, 187, 194, 92, 245, 171, 181, 45, 84, 19, 215, 216, 155, 25, 199, 70, 207, 158, 191, 19, 246, 151, 180, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 67, 69, 115, 55, 53, 161, 46, 0, 0, 193, 111, 242, 134, 35, 0, 186, 115, 227, 224, 148, 5, 0, 0, 0, 153, 245, 75, 1, 251, 254, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    // [0, 0, 0, 0, 0, 0, 1, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 35, 13, 4, 133, 109, 29, 198, 7, 44, 187, 194, 92, 245, 171, 181, 45, 84, 19, 215, 216, 155, 25, 199, 70, 207, 158, 191, 19, 246, 151, 180, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 67, 69, 115, 55, 53, 161, 46, 0, 0, 193, 111, 242, 134, 35, 0, 186, 115, 227, 224, 148, 5, 0, 0, 0, 153, 245, 75, 1, 251, 254, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    log::info!("{:?}", start_key);
    let key_prefix = [KeyPrefix::CheckPointIndex as u8];
    let mut cursor = index
        .open_cursor(
            Some(
                KeyRange::lower_bound(
                    &serde_wasm_bindgen::to_value(&start_key).unwrap(),
                    Some(false),
                )
                .unwrap()
                .into(),
            ),
            Some(idb::CursorDirection::NextUnique),
        )
        .unwrap()
        .await
        .unwrap()
        .unwrap()
        .into_managed();
    log::info!(
        "{:?}, {:?}",
        cursor.key().unwrap(),
        serde_wasm_bindgen::from_value::<KV>(cursor.value().unwrap().unwrap()).unwrap()
    );
    while let Ok(_) = cursor.next(None).await {
        if cursor.key().is_err() {
            break;
        }
        let key = cursor.key().unwrap();
        if key.is_none() {
            log::info!("finished");
            break;
        }
        log::info!(
            "{:?}, {:?}",
            key,
            serde_wasm_bindgen::from_value::<KV>(cursor.value().unwrap().unwrap()).unwrap()
        );
    }
}

#[wasm_bindgen]
pub async fn test_indexed_db() {
    use anyhow::Context;
    use indexed_db::Factory;

    utils::set_panic_hook();
    wasm_logger::init(wasm_logger::Config::new(log::Level::Info));

    let factory = Factory::<std::io::Error>::get()
        .context("opening IndexedDB")
        .unwrap();

    let db = factory
        .open("ckb-light-client", 1, |evt| async move {
            let db = evt.database();
            let store = db
                .build_object_store("data/store")
                .auto_increment()
                .create()?;
            store.build_index("key", "key").unique().create().unwrap();

            Ok(())
        })
        .await
        .context("creating the 'database' IndexedDB")
        .unwrap();

    db.transaction(&["data/store"])
        .rw()
        .run(|t| async move {
            let store = t.object_store("data/store")?;
            store
                .put_kv(
                    &serde_wasm_bindgen::to_value(&vec![
                        224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69,
                    ])
                    .unwrap(),
                    &serde_wasm_bindgen::to_value(&KV {
                        key: vec![224, 76, 65, 83, 84, 95, 83, 84, 65, 84, 69],
                        value: vec![
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 32, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 133, 109, 29, 198, 7, 44, 187, 194, 92, 245, 171, 181, 45,
                            84, 19, 215, 216, 155, 25, 199, 70, 207, 158, 191, 19, 246, 151, 180,
                            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 67, 69, 115, 55,
                            53, 161, 46, 0, 0, 193, 111, 242, 134, 35, 0, 186, 115, 227, 224, 148,
                            5, 0, 0, 0, 153, 245, 75, 1, 251, 254, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0,
                        ],
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            log::info!("2");
            Ok(())
        })
        .await
        .unwrap();

    db.transaction(&["data/store"])
        .run(|t| async move {
            let data = t.object_store("data/store")?.get_all(None).await.unwrap();
            log::info!("data len: {}", data.len());
            Ok(())
        })
        .await;
    log::info!("1");
}
