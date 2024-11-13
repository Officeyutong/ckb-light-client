use super::super::{
    extract_raw_data, parse_matched_blocks, BlockNumber, Byte32, CellIndex, CellType, CpIndex,
    HeaderWithExtension, Key, KeyPrefix, OutputIndex, Script, ScriptStatus, ScriptType,
    SetScriptsCommand, TxIndex, Value, WrappedBlockView, FILTER_SCRIPTS_KEY, GENESIS_BLOCK_KEY,
    LAST_N_HEADERS_KEY, LAST_STATE_KEY, MATCHED_FILTER_BLOCKS_KEY, MAX_CHECK_POINT_INDEX,
    MIN_FILTERED_BLOCK_NUMBER,
};
use crate::error::Result;
use ckb_network::runtime;
use ckb_types::{
    core::{
        cell::{CellMeta, CellStatus},
        HeaderView, TransactionInfo,
    },
    packed::{self, Block, CellOutput, Header, OutPoint, Transaction},
    prelude::*,
    utilities::{build_filter_data, calc_filter_hash},
    U256,
};
use idb::{
    DatabaseEvent, Factory, IndexParams, KeyPath, KeyRange, ManagedCursor, ObjectStore,
    ObjectStoreParams, TransactionMode, TransactionResult,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    usize,
};
use tokio::sync::mpsc::channel;

pub use idb::CursorDirection;

#[derive(Deserialize, Serialize, Debug)]
pub struct KV {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

struct Request {
    cmd: CommandRequest,
    resp: tokio::sync::oneshot::Sender<CommandResponse>,
}

enum CommandResponse {
    Read { values: Vec<Option<Vec<u8>>> },
    Put,
    Delete,
    Iterator { kvs: Vec<KV> },
    IteratorKey { keys: Vec<Vec<u8>> },
    Shutdown,
}

impl std::fmt::Debug for CommandResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandResponse::Delete => {
                write!(f, "Delete")
            }
            CommandResponse::Read { .. } => write!(f, "Read"),
            CommandResponse::Put { .. } => write!(f, "Put"),
            CommandResponse::Shutdown => write!(f, "Shutdown"),
            CommandResponse::Iterator { .. } => write!(f, "Iterator"),
            CommandResponse::IteratorKey { .. } => write!(f, "Iterator key"),
        }
    }
}

enum CommandRequest {
    Read {
        keys: Vec<Vec<u8>>,
    },
    Put {
        kvs: Vec<KV>,
    },
    Delete {
        keys: Vec<Vec<u8>>,
    },
    Iterator {
        start_key_bound: Vec<u8>,
        order: CursorDirection,
        take_while: Box<dyn Fn(&[u8]) -> bool + Send + 'static>,
        limit: usize,
        skip: usize,
    },
    IteratorKey {
        start_key_bound: Vec<u8>,
        order: CursorDirection,
        take_while: Box<dyn Fn(&[u8]) -> bool + Send + 'static>,
        limit: usize,
        skip: usize,
    },
    Shutdown,
}

impl std::fmt::Debug for CommandRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandRequest::Delete { .. } => {
                write!(f, "Delete")
            }
            CommandRequest::Read { .. } => write!(f, "Read"),
            CommandRequest::Put { .. } => write!(f, "Put"),
            CommandRequest::Shutdown => write!(f, "Shutdown"),
            CommandRequest::Iterator { .. } => write!(f, "Iterator"),
            CommandRequest::IteratorKey { .. } => write!(f, "Iterator key"),
        }
    }
}

#[derive(Clone)]
pub struct Storage {
    chan: tokio::sync::mpsc::Sender<Request>,
}

impl Storage {
    pub async fn new<P: AsRef<Path>>(path: P) -> Self {
        let factory = Factory::new().unwrap();
        let mut open_request = factory.open("ckb-light-client", Some(1)).unwrap();
        let store_name = path.as_ref().to_str().unwrap().to_owned();
        open_request.on_upgrade_needed(move |event| {
            let database = event.database().unwrap();
            let store_params = ObjectStoreParams::new();

            let store = database
                .create_object_store("data/store", store_params)
                .unwrap();
            let mut index_params = IndexParams::new();
            index_params.unique(true);
            store
                .create_index("key", KeyPath::new_single("key"), Some(index_params))
                .unwrap();
        });
        let db = open_request.await.unwrap();
        let (tx, mut rx) = channel(128);
        runtime::spawn(async move {
            loop {
                let request: Request = rx.recv().await.unwrap();
                match request.cmd {
                    CommandRequest::Read { keys } => {
                        let tran = db
                            .transaction(&[&store_name], TransactionMode::ReadOnly)
                            .unwrap();
                        let store = tran.object_store(&store_name).unwrap();
                        let mut res = Vec::new();
                        for key in keys {
                            let key = serde_wasm_bindgen::to_value(&key).unwrap();

                            res.push(
                                store.get(key).unwrap().await.unwrap().map(|v| {
                                    serde_wasm_bindgen::from_value::<KV>(v).unwrap().value
                                }),
                            );
                        }
                        assert_eq!(TransactionResult::Committed, tran.await.unwrap());
                        request
                            .resp
                            .send(CommandResponse::Read { values: res })
                            .unwrap()
                    }
                    CommandRequest::Put { kvs } => {
                        let tran = db
                            .transaction(&[&store_name], TransactionMode::ReadWrite)
                            .unwrap();
                        let store = tran.object_store(&store_name).unwrap();

                        for kv in kvs {
                            let key = serde_wasm_bindgen::to_value(&kv.key).unwrap();
                            let value = serde_wasm_bindgen::to_value(&kv).unwrap();
                            store.put(&value, Some(&key)).unwrap().await.unwrap();
                        }
                        assert_eq!(
                            TransactionResult::Committed,
                            tran.commit().unwrap().await.unwrap()
                        );
                        request.resp.send(CommandResponse::Put).unwrap();
                    }
                    CommandRequest::Delete { keys } => {
                        let tran = db
                            .transaction(&[&store_name], TransactionMode::ReadWrite)
                            .unwrap();
                        let store = tran.object_store(&store_name).unwrap();

                        for key in keys {
                            let key = serde_wasm_bindgen::to_value(&key).unwrap();
                            store.delete(key).unwrap().await.unwrap();
                        }
                        assert_eq!(
                            TransactionResult::Committed,
                            tran.commit().unwrap().await.unwrap()
                        );
                        request.resp.send(CommandResponse::Delete).unwrap();
                    }
                    CommandRequest::Shutdown => {
                        request.resp.send(CommandResponse::Shutdown).unwrap();
                        break;
                    }
                    CommandRequest::Iterator {
                        start_key_bound,
                        order,
                        take_while,
                        limit,
                        skip,
                    } => {
                        let tran = db
                            .transaction(&[&store_name], TransactionMode::ReadOnly)
                            .unwrap();
                        let store = tran.object_store(&store_name).unwrap();

                        let kvs = collect_iterator(
                            &store,
                            &start_key_bound,
                            order,
                            take_while,
                            limit,
                            skip,
                        )
                        .await
                        .unwrap();
                        assert_eq!(TransactionResult::Committed, tran.await.unwrap());
                        request
                            .resp
                            .send(CommandResponse::Iterator { kvs })
                            .unwrap();
                    }
                    CommandRequest::IteratorKey {
                        start_key_bound,
                        order,
                        take_while,
                        limit,
                        skip,
                    } => {
                        let tran = db
                            .transaction(&[&store_name], TransactionMode::ReadOnly)
                            .unwrap();
                        let store = tran.object_store(&store_name).unwrap();
                        let keys = collect_iterator_keys(
                            &store,
                            &start_key_bound,
                            order,
                            take_while,
                            limit,
                            skip,
                        )
                        .await
                        .unwrap();
                        assert_eq!(TransactionResult::Committed, tran.await.unwrap());
                        request
                            .resp
                            .send(CommandResponse::IteratorKey { keys })
                            .unwrap();
                    }
                }
            }
        });
        Self { chan: tx }
    }

    pub fn batch(&self) -> Batch {
        Batch {
            add: Vec::new(),
            delete: Vec::new(),
            chan: self.chan.clone(),
        }
    }

    pub async fn shutdown(&self) {
        if let CommandResponse::Shutdown = send_command(&self.chan, CommandRequest::Shutdown).await
        {
        } else {
            unreachable!()
        }
    }

    async fn put<K, V>(&self, key: K, value: V) -> Result<()>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let kv = KV {
            key: key.as_ref().to_vec(),
            value: value.as_ref().to_vec(),
        };

        send_command(&self.chan, CommandRequest::Put { kvs: vec![kv] }).await;
        Ok(())
    }

    pub async fn get<K: AsRef<[u8]>>(&self, key: K) -> Result<Option<Vec<u8>>> {
        let values = send_command(
            &self.chan,
            CommandRequest::Read {
                keys: vec![key.as_ref().to_vec()],
            },
        )
        .await;
        if let CommandResponse::Read { mut values } = values {
            return Ok(values.pop().unwrap());
        } else {
            unreachable!()
        }
    }

    async fn get_pinned<'a, K>(&'a self, key: K) -> Result<Option<Vec<u8>>>
    where
        K: AsRef<[u8]>,
    {
        self.get(key).await
    }

    async fn delete<K: AsRef<[u8]>>(&self, key: K) -> Result<()> {
        send_command(
            &self.chan,
            CommandRequest::Delete {
                keys: vec![key.as_ref().to_vec()],
            },
        )
        .await;
        Ok(())
    }

    pub async fn is_filter_scripts_empty_async(&self) -> bool {
        let key_prefix = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();

        let value = send_command(
            &self.chan,
            CommandRequest::IteratorKey {
                start_key_bound: key_prefix.clone(),
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix)),
                limit: 1,
                skip: 0,
            },
        )
        .await;

        if let CommandResponse::IteratorKey { keys } = value {
            keys.is_empty()
        } else {
            unreachable!()
        }
    }

    pub async fn get_filter_scripts(&self) -> Vec<ScriptStatus> {
        let key_prefix = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();
        let key_prefix_clone = key_prefix.clone();
        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound: key_prefix_clone.clone(),
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix_clone)),
                limit: 1,
                skip: 0,
            },
        )
        .await;

        if let CommandResponse::Iterator { kvs } = value {
            kvs.into_iter()
                .map(|kv| (kv.key, kv.value))
                .map(|(key, value)| {
                    let script = Script::from_slice(&key[key_prefix.len()..key.len() - 1])
                        .expect("stored Script");
                    let script_type = match key[key.len() - 1] {
                        0 => ScriptType::Lock,
                        1 => ScriptType::Type,
                        _ => panic!("invalid script type"),
                    };
                    let block_number = BlockNumber::from_be_bytes(
                        AsRef::<[u8]>::as_ref(&value)
                            .try_into()
                            .expect("stored BlockNumber"),
                    );
                    ScriptStatus {
                        script,
                        script_type,
                        block_number,
                    }
                })
                .collect()
        } else {
            unreachable!()
        }
    }

    pub async fn update_filter_scripts(
        &self,
        scripts: Vec<ScriptStatus>,
        command: SetScriptsCommand,
    ) {
        let mut should_filter_genesis_block = false;
        let mut batch = self.batch();
        let key_prefix = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();

        match command {
            SetScriptsCommand::All => {
                should_filter_genesis_block = scripts.iter().any(|ss| ss.block_number == 0);

                let key_prefix_clone = key_prefix.clone();
                let remove_keys = send_command(
                    &self.chan,
                    CommandRequest::IteratorKey {
                        start_key_bound: key_prefix_clone.clone(),
                        order: CursorDirection::NextUnique,
                        take_while: Box::new(move |raw_key: &[u8]| {
                            raw_key.starts_with(&key_prefix_clone)
                        }),
                        limit: usize::MAX,
                        skip: 0,
                    },
                )
                .await;

                if let CommandResponse::IteratorKey { keys } = remove_keys {
                    batch.delete_many(keys).await.unwrap();
                } else {
                    unreachable!()
                }

                for ss in scripts {
                    let key = [
                        key_prefix.as_ref(),
                        ss.script.as_slice(),
                        match ss.script_type {
                            ScriptType::Lock => &[0],
                            ScriptType::Type => &[1],
                        },
                    ]
                    .concat();
                    batch
                        .put(key, ss.block_number.to_be_bytes())
                        .expect("batch put should be ok");
                }
            }
            SetScriptsCommand::Partial => {
                if scripts.is_empty() {
                    return;
                }
                let min_script_block_number = scripts.iter().map(|ss| ss.block_number).min();
                should_filter_genesis_block = min_script_block_number == Some(0);

                for ss in scripts {
                    let key = [
                        key_prefix.as_ref(),
                        ss.script.as_slice(),
                        match ss.script_type {
                            ScriptType::Lock => &[0],
                            ScriptType::Type => &[1],
                        },
                    ]
                    .concat();
                    batch
                        .put(key, ss.block_number.to_be_bytes())
                        .expect("batch put should be ok");
                }
            }
            SetScriptsCommand::Delete => {
                if scripts.is_empty() {
                    return;
                }

                for ss in scripts {
                    let key = [
                        key_prefix.as_ref(),
                        ss.script.as_slice(),
                        match ss.script_type {
                            ScriptType::Lock => &[0],
                            ScriptType::Type => &[1],
                        },
                    ]
                    .concat();
                    batch.delete(key).expect("batch delete should be ok");
                }
            }
        }

        batch.commit().await.expect("batch commit should be ok");

        self.update_min_filtered_block_number_by_scripts().await;
        self.clear_matched_blocks().await;

        if should_filter_genesis_block {
            let block = self.get_genesis_block_async().await;
            self.filter_block_async(block).await;
        }
    }

    async fn update_min_filtered_block_number_by_scripts(&self) {
        let key_prefix = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();

        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound: key_prefix.clone(),
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix)),
                limit: usize::MAX,
                skip: 0,
            },
        )
        .await;

        if let CommandResponse::Iterator { kvs } = value {
            let min_block_number = kvs
                .into_iter()
                .map(|kv| (kv.key, kv.value))
                .map(|(_key, value)| {
                    BlockNumber::from_be_bytes(
                        AsRef::<[u8]>::as_ref(&value)
                            .try_into()
                            .expect("stored BlockNumber"),
                    )
                })
                .min();

            if let Some(n) = min_block_number {
                self.update_min_filtered_block_number_async(n).await;
            }
        } else {
            unreachable!()
        }
    }

    pub async fn get_scripts_hash_async(&self, block_number: BlockNumber) -> Vec<Byte32> {
        let key_prefix = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();

        let key_prefix_clone = key_prefix.clone();
        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound: key_prefix_clone.clone(),
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix_clone)),
                limit: usize::MAX,
                skip: 0,
            },
        )
        .await;

        if let CommandResponse::Iterator { kvs } = value {
            kvs.into_iter()
                .map(|kv| (kv.key, kv.value))
                .filter_map(|(key, value)| {
                    let stored_block_number = BlockNumber::from_be_bytes(
                        AsRef::<[u8]>::as_ref(&value)
                            .try_into()
                            .expect("stored BlockNumber"),
                    );
                    if stored_block_number < block_number {
                        let script = Script::from_slice(&key[key_prefix.len()..key.len() - 1])
                            .expect("stored Script");
                        Some(script.calc_script_hash())
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            unreachable!()
        }
    }

    async fn clear_matched_blocks(&self) {
        let key_prefix = Key::Meta(MATCHED_FILTER_BLOCKS_KEY).into_vec();

        let mut batch = self.batch();

        let value = send_command(
            &self.chan,
            CommandRequest::IteratorKey {
                start_key_bound: key_prefix.clone(),
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix)),
                limit: usize::MAX,
                skip: 0,
            },
        )
        .await;
        if let CommandResponse::IteratorKey { keys } = value {
            batch.delete_many(keys).await.unwrap();
            batch.commit().await.unwrap();
        } else {
            unreachable!()
        }
    }

    async fn get_matched_blocks(
        &self,
        direction: CursorDirection,
    ) -> Option<(u64, u64, Vec<(Byte32, bool)>)> {
        let key_prefix = Key::Meta(MATCHED_FILTER_BLOCKS_KEY).into_vec();
        let iter_from = match direction {
            CursorDirection::NextUnique => key_prefix.clone(),
            CursorDirection::PrevUnique => {
                let mut key = key_prefix.clone();
                key.extend(u64::max_value().to_be_bytes());
                key
            }
            _ => panic!("Invalid direction"),
        };

        let key_prefix_clone = key_prefix.clone();

        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound: iter_from,
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix_clone)),
                limit: 1,
                skip: 0,
            },
        )
        .await;
        if let CommandResponse::Iterator { kvs } = value {
            kvs.into_iter()
                .map(|kv| (kv.key, kv.value))
                .map(|(key, value)| {
                    let mut u64_bytes = [0u8; 8];
                    u64_bytes.copy_from_slice(&key[key_prefix.len()..]);
                    let start_number = u64::from_be_bytes(u64_bytes);
                    let (blocks_count, blocks) = parse_matched_blocks(&value);
                    (start_number, blocks_count, blocks)
                })
                .next()
        } else {
            unreachable!()
        }
    }

    pub async fn get_earliest_matched_blocks_async(
        &self,
    ) -> Option<(u64, u64, Vec<(Byte32, bool)>)> {
        self.get_matched_blocks(CursorDirection::NextUnique).await
    }

    pub async fn get_latest_matched_blocks_async(&self) -> Option<(u64, u64, Vec<(Byte32, bool)>)> {
        self.get_matched_blocks(CursorDirection::PrevUnique).await
    }

    pub async fn get_check_points_async(&self, start_index: CpIndex, limit: usize) -> Vec<Byte32> {
        let start_key = Key::CheckPointIndex(start_index).into_vec();
        let key_prefix = [KeyPrefix::CheckPointIndex as u8];

        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound: start_key,
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix)),
                limit,
                skip: 0,
            },
        )
        .await;

        if let CommandResponse::Iterator { kvs } = value {
            kvs.into_iter()
                .map(|kv| (kv.key, kv.value))
                .map(|(_key, value)| Byte32::from_slice(&value).expect("stored block filter hash"))
                .collect()
        } else {
            unreachable!()
        }
    }

    pub async fn update_block_number_async(&self, block_number: BlockNumber) {
        let key_prefix = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();
        let mut batch = self.batch();

        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound: key_prefix.clone(),
                order: CursorDirection::NextUnique,
                take_while: Box::new(move |raw_key: &[u8]| raw_key.starts_with(&key_prefix)),
                limit: usize::MAX,
                skip: 0,
            },
        )
        .await;

        if let CommandResponse::Iterator { kvs } = value {
            kvs.into_iter()
                .map(|kv| (kv.key, kv.value))
                .for_each(|(key, value)| {
                    let stored_block_number = BlockNumber::from_be_bytes(
                        AsRef::<[u8]>::as_ref(&value)
                            .try_into()
                            .expect("stored BlockNumber"),
                    );
                    if stored_block_number < block_number {
                        batch
                            .put(key, block_number.to_be_bytes())
                            .expect("batch put should be ok")
                    }
                });
            batch.commit().await.expect("batch commit should be ok");
        }
    }

    pub async fn rollback_to_block_async(&self, to_number: BlockNumber) {
        let scripts = self.get_filter_scripts().await;
        let mut batch = self.batch();

        for ss in scripts {
            if ss.block_number >= to_number {
                let script = ss.script;
                let mut key_prefix = vec![match ss.script_type {
                    ScriptType::Lock => KeyPrefix::TxLockScript as u8,
                    ScriptType::Type => KeyPrefix::TxTypeScript as u8,
                }];
                key_prefix.extend_from_slice(&extract_raw_data(&script));
                let mut start_key = key_prefix.clone();
                start_key.extend_from_slice(BlockNumber::MAX.to_be_bytes().as_ref());
                let key_prefix_len = key_prefix.len();

                let value = send_command(
                    &self.chan,
                    CommandRequest::Iterator {
                        start_key_bound: key_prefix.clone(),
                        order: CursorDirection::PrevUnique,
                        take_while: Box::new(move |raw_key: &[u8]| {
                            raw_key.starts_with(&key_prefix)
                                && BlockNumber::from_be_bytes(
                                    raw_key[key_prefix_len..key_prefix_len + 8]
                                        .try_into()
                                        .expect("stored BlockNumber"),
                                ) >= to_number
                        }),
                        limit: usize::MAX,
                        skip: 0,
                    },
                )
                .await;

                if let CommandResponse::Iterator { kvs } = value {
                    for (key, value) in kvs.into_iter().map(|kv| (kv.key, kv.value)) {
                        let block_number = BlockNumber::from_be_bytes(
                            key[key_prefix_len..key_prefix_len + 8]
                                .try_into()
                                .expect("stored BlockNumber"),
                        );
                        log::debug!("rollback {}", block_number);
                        let tx_index = TxIndex::from_be_bytes(
                            key[key_prefix_len + 8..key_prefix_len + 12]
                                .try_into()
                                .expect("stored TxIndex"),
                        );
                        let cell_index = CellIndex::from_be_bytes(
                            key[key_prefix_len + 12..key_prefix_len + 16]
                                .try_into()
                                .expect("stored CellIndex"),
                        );
                        let tx_hash =
                            packed::Byte32Reader::from_slice_should_be_ok(&value).to_entity();
                        if key[key_prefix_len + 16] == 0 {
                            let (_, _, tx) = self
                                .get_transaction(&tx_hash)
                                .await
                                .expect("stored transaction history");
                            let input = tx.raw().inputs().get(cell_index as usize).unwrap();
                            if let Some((
                                generated_by_block_number,
                                generated_by_tx_index,
                                _previous_tx,
                            )) = self
                                .get_transaction(&input.previous_output().tx_hash())
                                .await
                            {
                                let key = match ss.script_type {
                                    ScriptType::Lock => Key::CellLockScript(
                                        &script,
                                        generated_by_block_number,
                                        generated_by_tx_index,
                                        input.previous_output().index().unpack(),
                                    ),
                                    ScriptType::Type => Key::CellTypeScript(
                                        &script,
                                        generated_by_block_number,
                                        generated_by_tx_index,
                                        input.previous_output().index().unpack(),
                                    ),
                                };
                                batch
                                    .put_kv(key, input.previous_output().tx_hash().as_slice())
                                    .expect("batch put should be ok");
                            };
                            // delete tx history
                            let key = match ss.script_type {
                                ScriptType::Lock => Key::TxLockScript(
                                    &script,
                                    block_number,
                                    tx_index,
                                    cell_index,
                                    CellType::Input,
                                ),
                                ScriptType::Type => Key::TxTypeScript(
                                    &script,
                                    block_number,
                                    tx_index,
                                    cell_index,
                                    CellType::Input,
                                ),
                            }
                            .into_vec();
                            batch.delete(key).expect("batch delete should be ok");
                        } else {
                            // delete utxo
                            let key = match ss.script_type {
                                ScriptType::Lock => {
                                    Key::CellLockScript(&script, block_number, tx_index, cell_index)
                                }
                                ScriptType::Type => {
                                    Key::CellTypeScript(&script, block_number, tx_index, cell_index)
                                }
                            }
                            .into_vec();
                            batch.delete(key).expect("batch delete should be ok");

                            // delete tx history
                            let key = match ss.script_type {
                                ScriptType::Lock => Key::TxLockScript(
                                    &script,
                                    block_number,
                                    tx_index,
                                    cell_index,
                                    CellType::Output,
                                ),
                                ScriptType::Type => Key::TxTypeScript(
                                    &script,
                                    block_number,
                                    tx_index,
                                    cell_index,
                                    CellType::Output,
                                ),
                            }
                            .into_vec();
                            batch.delete(key).expect("batch delete should be ok");
                        };
                    }

                    // update script filter block number
                    {
                        let mut key = Key::Meta(FILTER_SCRIPTS_KEY).into_vec();
                        key.extend_from_slice(script.as_slice());
                        key.extend_from_slice(match ss.script_type {
                            ScriptType::Lock => &[0],
                            ScriptType::Type => &[1],
                        });
                        let value = to_number.to_be_bytes().to_vec();
                        batch.put(key, value).expect("batch put should be ok");
                    }
                } else {
                    unreachable!()
                }
            }
        }
        // we should also sync block filters again
        if self.get_min_filtered_block_number_async().await >= to_number {
            batch
                .put(
                    Key::Meta(MIN_FILTERED_BLOCK_NUMBER).into_vec(),
                    to_number.saturating_sub(1).to_le_bytes(),
                )
                .expect("batch put should be ok");
        }

        batch.commit().await.expect("batch commit should be ok");
    }
}

pub struct Batch {
    add: Vec<KV>,
    delete: Vec<Vec<u8>>,
    chan: tokio::sync::mpsc::Sender<Request>,
}

impl Batch {
    fn put_kv<K: Into<Vec<u8>>, V: Into<Vec<u8>>>(&mut self, key: K, value: V) -> Result<()> {
        self.add.push(KV {
            key: key.into(),
            value: value.into(),
        });
        Ok(())
    }

    fn put<K: AsRef<[u8]>, V: AsRef<[u8]>>(&mut self, key: K, value: V) -> Result<()> {
        self.add.push(KV {
            key: key.as_ref().to_vec(),
            value: value.as_ref().to_vec(),
        });
        Ok(())
    }

    fn delete<K: AsRef<[u8]>>(&mut self, key: K) -> Result<()> {
        self.delete.push(key.as_ref().to_vec());
        Ok(())
    }

    async fn delete_many(&mut self, keys: Vec<Vec<u8>>) -> Result<()> {
        send_command(&self.chan, CommandRequest::Delete { keys }).await;
        Ok(())
    }

    async fn commit(self) -> Result<()> {
        if !self.add.is_empty() {
            send_command(&self.chan, CommandRequest::Put { kvs: self.add }).await;
        }

        if !self.delete.is_empty() {
            send_command(&self.chan, CommandRequest::Delete { keys: self.delete }).await;
        }

        Ok(())
    }
}

async fn send_command(
    chan: &tokio::sync::mpsc::Sender<Request>,
    cmd: CommandRequest,
) -> CommandResponse {
    let (tx, rx) = tokio::sync::oneshot::channel();
    chan.send(Request { cmd, resp: tx }).await.unwrap();
    rx.await.unwrap()
}

impl Storage {
    pub async fn collect_iterator(
        &self,
        start_key_bound: Vec<u8>,
        order: CursorDirection,
        take_while: Box<dyn Fn(&[u8]) -> bool + Send + 'static>,
        limit: usize,
        skip: usize,
    ) -> Vec<KV> {
        let value = send_command(
            &self.chan,
            CommandRequest::Iterator {
                start_key_bound,
                order,
                take_while,
                limit,
                skip,
            },
        )
        .await;
        if let CommandResponse::Iterator { kvs } = value {
            return kvs;
        } else {
            unreachable!()
        }
    }
    pub async fn init_genesis_block(&self, block: Block) {
        let genesis_hash = block.calc_header_hash();
        let genesis_block_key = Key::Meta(GENESIS_BLOCK_KEY).into_vec();
        if let Some(stored_genesis_hash) = self
            .get(genesis_block_key.as_slice())
            .await
            .expect("get genesis block")
            .map(|v| v[0..32].to_vec())
        {
            if genesis_hash.as_slice() != stored_genesis_hash.as_slice() {
                panic!(
                    "genesis hash mismatch: stored={:#?}, new={}",
                    stored_genesis_hash, genesis_hash
                );
            }
        } else {
            let mut batch = self.batch();
            let block_hash = block.calc_header_hash();
            batch
                .put_kv(Key::Meta(LAST_STATE_KEY), block.header().as_slice())
                .expect("batch put should be ok");
            batch
                .put_kv(Key::BlockHash(&block_hash), block.header().as_slice())
                .expect("batch put should be ok");
            batch
                .put_kv(Key::BlockNumber(0), block_hash.as_slice())
                .expect("batch put should be ok");
            let mut genesis_hash_and_txs_hash = genesis_hash.as_slice().to_vec();
            block
                .transactions()
                .into_iter()
                .enumerate()
                .for_each(|(tx_index, tx)| {
                    let tx_hash = tx.calc_tx_hash();
                    genesis_hash_and_txs_hash.extend_from_slice(tx_hash.as_slice());
                    let key = Key::TxHash(&tx_hash).into_vec();
                    let value = Value::Transaction(0, tx_index as TxIndex, &tx);
                    batch.put_kv(key, value).expect("batch put should be ok");
                });
            batch
                .put_kv(genesis_block_key, genesis_hash_and_txs_hash.as_slice())
                .expect("batch put should be ok");
            batch.commit().await.expect("batch commit should be ok");
            self.update_last_state_async(&U256::zero(), &block.header(), &[])
                .await;
            let genesis_block_filter_hash: Byte32 = {
                let block_view = block.into_view();
                let provider = WrappedBlockView::new(&block_view);
                let parent_block_filter_hash = Byte32::zero();
                let (genesis_block_filter_vec, missing_out_points) =
                    build_filter_data(provider, &block_view.transactions());
                if !missing_out_points.is_empty() {
                    panic!("Genesis block shouldn't missing any out points.");
                }
                let genesis_block_filter_data = genesis_block_filter_vec.pack();
                calc_filter_hash(&parent_block_filter_hash, &genesis_block_filter_data).pack()
            };
            self.update_max_check_point_index_async(0).await;
            self.update_check_points_async(0, &[genesis_block_filter_hash])
                .await;
            self.update_min_filtered_block_number_async(0).await;
        }
    }

    pub async fn get_genesis_block_async(&self) -> Block {
        let genesis_hash_and_txs_hash = self
            .get(Key::Meta(GENESIS_BLOCK_KEY).into_vec())
            .await
            .expect("get genesis block")
            .expect("inited storage");
        let genesis_hash = Byte32::from_slice(&genesis_hash_and_txs_hash[0..32])
            .expect("stored genesis block hash");
        let genesis_header = Header::from_slice(
            &self
                .get(Key::BlockHash(&genesis_hash).into_vec())
                .await
                .expect("db get should be ok")
                .expect("stored block hash / header mapping"),
        )
        .expect("stored header should be OK");

        let mut transactions: Vec<Transaction> = Vec::new();
        for tx_hash in genesis_hash_and_txs_hash[32..].chunks_exact(32) {
            transactions.push(
                Transaction::from_slice(
                    &self
                        .get(
                            Key::TxHash(
                                &Byte32::from_slice(tx_hash).expect("stored genesis block tx hash"),
                            )
                            .into_vec(),
                        )
                        .await
                        .expect("db get should be ok")
                        .expect("stored genesis block tx")[12..],
                )
                .expect("stored Transaction"),
            )
        }

        Block::new_builder()
            .header(genesis_header)
            .transactions(transactions.pack())
            .build()
    }

    pub async fn update_last_state_async(
        &self,
        total_difficulty: &U256,
        tip_header: &Header,
        last_n_headers: &[HeaderView],
    ) {
        let key = Key::Meta(LAST_STATE_KEY).into_vec();
        let mut value = total_difficulty.to_le_bytes().to_vec();
        value.extend(tip_header.as_slice());
        self.put(key, &value)
            .await
            .expect("db put last state should be ok");
        self.update_last_n_headers(last_n_headers).await;
    }

    pub async fn get_last_state_async(&self) -> (U256, Header) {
        let key = Key::Meta(LAST_STATE_KEY).into_vec();
        self.get_pinned(&key)
            .await
            .expect("db get last state should be ok")
            .map(|data| {
                let mut total_difficulty_bytes = [0u8; 32];
                total_difficulty_bytes.copy_from_slice(&data[0..32]);
                let total_difficulty = U256::from_le_bytes(&total_difficulty_bytes);
                let header = packed::HeaderReader::from_slice_should_be_ok(&data[32..]).to_entity();
                (total_difficulty, header)
            })
            .expect("tip header should be inited")
    }

    async fn update_last_n_headers(&self, headers: &[HeaderView]) {
        let key = Key::Meta(LAST_N_HEADERS_KEY).into_vec();
        let mut value: Vec<u8> = Vec::with_capacity(headers.len() * 40);
        for header in headers {
            value.extend(header.number().to_le_bytes());
            value.extend(header.hash().as_slice());
        }
        self.put(key, &value)
            .await
            .expect("db put last n headers should be ok");
    }

    pub async fn get_last_n_headers_async(&self) -> Vec<(u64, Byte32)> {
        let key = Key::Meta(LAST_N_HEADERS_KEY).into_vec();
        self.get_pinned(&key)
            .await
            .expect("db get last n headers should be ok")
            .map(|data| {
                assert!(data.len() % 40 == 0);
                let mut headers = Vec::with_capacity(data.len() / 40);
                for part in data.chunks(40) {
                    let number = u64::from_le_bytes(part[0..8].try_into().unwrap());
                    let hash = Byte32::from_slice(&part[8..]).expect("byte32 block hash");
                    headers.push((number, hash));
                }
                headers
            })
            .expect("last n headers should be inited")
    }

    pub async fn remove_matched_blocks_async(&self, start_number: u64) {
        let mut key = Key::Meta(MATCHED_FILTER_BLOCKS_KEY).into_vec();
        key.extend(start_number.to_be_bytes());
        self.delete(&key).await.expect("delete matched blocks");
    }

    pub async fn add_matched_blocks_async(
        &self,
        start_number: u64,
        blocks_count: u64,
        // (block-hash, proved)
        matched_blocks: Vec<(Byte32, bool)>,
    ) {
        assert!(!matched_blocks.is_empty());
        let mut key = Key::Meta(MATCHED_FILTER_BLOCKS_KEY).into_vec();
        key.extend(start_number.to_be_bytes());

        let mut value = blocks_count.to_le_bytes().to_vec();
        for (block_hash, proved) in matched_blocks {
            value.extend(block_hash.as_slice());
            value.push(u8::from(proved));
        }
        self.put(key, &value)
            .await
            .expect("db put matched blocks should be ok");
    }

    pub async fn add_fetched_header_async(&self, hwe: &HeaderWithExtension) {
        let mut batch = self.batch();
        let block_hash = hwe.header.calc_header_hash();
        batch
            .put(Key::BlockHash(&block_hash).into_vec(), hwe.to_vec())
            .expect("batch put should be ok");
        batch
            .put(
                Key::BlockNumber(hwe.header.raw().number().unpack()).into_vec(),
                block_hash.as_slice(),
            )
            .expect("batch put should be ok");
        batch.commit().await.expect("batch commit should be ok");
    }

    pub async fn add_fetched_tx_async(&self, tx: &Transaction, hwe: &HeaderWithExtension) {
        let mut batch = self.batch();
        let block_hash = hwe.header.calc_header_hash();
        let block_number: u64 = hwe.header.raw().number().unpack();
        batch
            .put(Key::BlockHash(&block_hash).into_vec(), hwe.to_vec())
            .expect("batch put should be ok");
        batch
            .put(
                Key::BlockNumber(block_number).into_vec(),
                block_hash.as_slice(),
            )
            .expect("batch put should be ok");
        let tx_hash = tx.calc_tx_hash();
        let tx_index = u32::max_value();
        let key = Key::TxHash(&tx_hash).into_vec();
        let value = Value::Transaction(block_number, tx_index as TxIndex, tx);
        batch.put_kv(key, value).expect("batch put should be ok");
        batch.commit().await.expect("batch commit should be ok");
    }

    pub async fn get_tip_header_async(&self) -> Header {
        self.get_tip_header().await
    }

    pub async fn get_tip_header(&self) -> Header {
        self.get_last_state_async().await.1
    }

    pub async fn get_min_filtered_block_number_async(&self) -> BlockNumber {
        let key = Key::Meta(MIN_FILTERED_BLOCK_NUMBER).into_vec();
        self.get_pinned(&key)
            .await
            .expect("db get min filtered block number should be ok")
            .map(|data| u64::from_le_bytes(AsRef::<[u8]>::as_ref(&data).try_into().unwrap()))
            .unwrap_or_default()
    }

    pub async fn update_min_filtered_block_number_async(&self, block_number: BlockNumber) {
        let key = Key::Meta(MIN_FILTERED_BLOCK_NUMBER).into_vec();
        let value = block_number.to_le_bytes();
        self.put(key, value)
            .await
            .expect("db put min filtered block number should be ok");
    }

    pub async fn get_last_check_point_async(&self) -> (CpIndex, Byte32) {
        let index = self.get_max_check_point_index_async().await;
        let hash = self
            .get_check_points_async(index, 1)
            .await
            .get(0)
            .cloned()
            .expect("db get last check point should be ok");
        (index, hash)
    }

    pub async fn get_max_check_point_index_async(&self) -> CpIndex {
        let key = Key::Meta(MAX_CHECK_POINT_INDEX).into_vec();
        self.get_pinned(&key)
            .await
            .expect("db get max check point index should be ok")
            .map(|data| CpIndex::from_be_bytes(AsRef::<[u8]>::as_ref(&data).try_into().unwrap()))
            .expect("db get max check point index should be ok 1")
    }

    pub async fn update_max_check_point_index_async(&self, index: CpIndex) {
        let key = Key::Meta(MAX_CHECK_POINT_INDEX).into_vec();
        let value = index.to_be_bytes();
        self.put(key, value)
            .await
            .expect("db put max check point index should be ok");
    }

    pub async fn update_check_points_async(&self, start_index: CpIndex, check_points: &[Byte32]) {
        let mut index = start_index;
        let mut batch = self.batch();
        for cp in check_points {
            let key = Key::CheckPointIndex(index).into_vec();
            let value = Value::BlockFilterHash(cp);
            batch.put_kv(key, value).expect("batch put should be ok");
            index += 1;
        }
        batch.commit().await.expect("batch commit should be ok");
    }

    pub async fn filter_block_async(&self, block: Block) {
        let scripts: HashSet<(Script, ScriptType)> = self
            .get_filter_scripts()
            .await
            .into_iter()
            .map(|ss| (ss.script, ss.script_type))
            .collect();
        let block_number: BlockNumber = block.header().raw().number().unpack();
        let mut filter_matched = false;
        let mut batch = self.batch();
        let mut txs: HashMap<Byte32, (u32, Transaction)> = HashMap::new();
        for (tx_index, tx) in block.transactions().into_iter().enumerate() {
            for (input_index, input) in tx.raw().inputs().into_iter().enumerate() {
                let previous_tx_hash = input.previous_output().tx_hash();
                if let Some((generated_by_block_number, generated_by_tx_index, previous_tx)) =
                    self.get_transaction(&previous_tx_hash).await.or(txs
                        .get(&previous_tx_hash)
                        .map(|(tx_index, tx)| (block_number, *tx_index, tx.clone())))
                {
                    let previous_output_index = input.previous_output().index().unpack();
                    if let Some(previous_output) =
                        previous_tx.raw().outputs().get(previous_output_index)
                    {
                        let script = previous_output.lock();
                        if scripts.contains(&(script.clone(), ScriptType::Lock)) {
                            filter_matched = true;
                            // delete utxo
                            let key = Key::CellLockScript(
                                &script,
                                generated_by_block_number,
                                generated_by_tx_index,
                                previous_output_index as OutputIndex,
                            )
                            .into_vec();
                            batch.delete(key).expect("batch delete should be ok");
                            // insert tx history
                            let key = Key::TxLockScript(
                                &script,
                                block_number,
                                tx_index as TxIndex,
                                input_index as CellIndex,
                                CellType::Input,
                            )
                            .into_vec();
                            let tx_hash = tx.calc_tx_hash();
                            batch
                                .put(key, tx_hash.as_slice())
                                .expect("batch put should be ok");
                            // insert tx
                            let key = Key::TxHash(&tx_hash).into_vec();
                            let value = Value::Transaction(block_number, tx_index as TxIndex, &tx);
                            batch.put_kv(key, value).expect("batch put should be ok");
                        }
                        if let Some(script) = previous_output.type_().to_opt() {
                            if scripts.contains(&(script.clone(), ScriptType::Type)) {
                                filter_matched = true;
                                // delete utxo
                                let key = Key::CellTypeScript(
                                    &script,
                                    generated_by_block_number,
                                    generated_by_tx_index,
                                    previous_output_index as OutputIndex,
                                )
                                .into_vec();
                                batch.delete(key).expect("batch delete should be ok");
                                // insert tx history
                                let key = Key::TxTypeScript(
                                    &script,
                                    block_number,
                                    tx_index as TxIndex,
                                    input_index as CellIndex,
                                    CellType::Input,
                                )
                                .into_vec();
                                let tx_hash = tx.calc_tx_hash();
                                batch
                                    .put(key, tx_hash.as_slice())
                                    .expect("batch put should be ok");
                                // insert tx
                                let key = Key::TxHash(&tx_hash).into_vec();
                                let value =
                                    Value::Transaction(block_number, tx_index as TxIndex, &tx);
                                batch.put_kv(key, value).expect("batch put should be ok");
                            }
                        }
                    }
                }
            }

            tx.raw()
                .outputs()
                .into_iter()
                .enumerate()
                .for_each(|(output_index, output)| {
                    let script = output.lock();
                    if scripts.contains(&(script.clone(), ScriptType::Lock)) {
                        filter_matched = true;
                        let tx_hash = tx.calc_tx_hash();
                        // insert utxo
                        let key = Key::CellLockScript(
                            &script,
                            block_number,
                            tx_index as TxIndex,
                            output_index as OutputIndex,
                        )
                        .into_vec();
                        batch
                            .put(key, tx_hash.as_slice())
                            .expect("batch put should be ok");
                        // insert tx history
                        let key = Key::TxLockScript(
                            &script,
                            block_number,
                            tx_index as TxIndex,
                            output_index as CellIndex,
                            CellType::Output,
                        )
                        .into_vec();
                        batch
                            .put(key, tx_hash.as_slice())
                            .expect("batch put should be ok");
                        // insert tx
                        let key = Key::TxHash(&tx_hash).into_vec();
                        let value = Value::Transaction(block_number, tx_index as TxIndex, &tx);
                        batch.put_kv(key, value).expect("batch put should be ok");
                    }
                    if let Some(script) = output.type_().to_opt() {
                        if scripts.contains(&(script.clone(), ScriptType::Type)) {
                            filter_matched = true;
                            let tx_hash = tx.calc_tx_hash();
                            // insert utxo
                            let key = Key::CellTypeScript(
                                &script,
                                block_number,
                                tx_index as TxIndex,
                                output_index as OutputIndex,
                            )
                            .into_vec();
                            batch
                                .put(key, tx_hash.as_slice())
                                .expect("batch put should be ok");
                            // insert tx history
                            let key = Key::TxTypeScript(
                                &script,
                                block_number,
                                tx_index as TxIndex,
                                output_index as CellIndex,
                                CellType::Output,
                            )
                            .into_vec();
                            batch
                                .put(key, tx_hash.as_slice())
                                .expect("batch put should be ok");
                            // insert tx
                            let key = Key::TxHash(&tx_hash).into_vec();
                            let value = Value::Transaction(block_number, tx_index as TxIndex, &tx);
                            batch.put_kv(key, value).expect("batch put should be ok");
                        }
                    }
                });

            txs.insert(tx.calc_tx_hash(), (tx_index as u32, tx));
        }
        if filter_matched {
            let block_hash = block.calc_header_hash();
            let hwe = HeaderWithExtension {
                header: block.header(),
                extension: block.extension(),
            };
            batch
                .put(Key::BlockHash(&block_hash).into_vec(), hwe.to_vec())
                .expect("batch put should be ok");
            batch
                .put(
                    Key::BlockNumber(block.header().raw().number().unpack()).into_vec(),
                    block_hash.as_slice(),
                )
                .expect("batch put should be ok");
        }
        batch.commit().await.expect("batch commit should be ok");
    }

    async fn get_transaction(
        &self,
        tx_hash: &Byte32,
    ) -> Option<(BlockNumber, TxIndex, Transaction)> {
        self.get(Key::TxHash(tx_hash).into_vec())
            .await
            .map(|v| {
                v.map(|v| {
                    (
                        BlockNumber::from_be_bytes(v[0..8].try_into().expect("stored BlockNumber")),
                        TxIndex::from_be_bytes(v[8..12].try_into().expect("stored TxIndex")),
                        Transaction::from_slice(&v[12..]).expect("stored Transaction"),
                    )
                })
            })
            .expect("db get should be ok")
    }

    pub async fn get_transaction_with_header(
        &self,
        tx_hash: &Byte32,
    ) -> Option<(Transaction, Header)> {
        match self.get_transaction(tx_hash).await {
            Some((block_number, _tx_index, tx)) => {
                let block_hash = Byte32::from_slice(
                    &self
                        .get(Key::BlockNumber(block_number).into_vec())
                        .await
                        .expect("db get should be ok")
                        .expect("stored block number / hash mapping"),
                )
                .expect("stored block hash should be OK");

                let header = Header::from_slice(
                    &self
                        .get(Key::BlockHash(&block_hash).into_vec())
                        .await
                        .expect("db get should be ok")
                        .expect("stored block hash / header mapping")[..Header::TOTAL_SIZE],
                )
                .expect("stored header should be OK");
                Some((tx, header))
            }
            None => None,
        }
    }

    pub async fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        if let Some((block_number, tx_index, tx)) = self.get_transaction(&out_point.tx_hash()).await
        {
            let block_hash = Byte32::from_slice(
                &self
                    .get(Key::BlockNumber(block_number).into_vec())
                    .await
                    .expect("db get should be ok")
                    .expect("stored block number / hash mapping"),
            )
            .expect("stored block hash should be OK");

            let header = Header::from_slice(
                &self
                    .get(Key::BlockHash(&block_hash).into_vec())
                    .await
                    .expect("db get should be ok")
                    .expect("stored block hash / header mapping")[..Header::TOTAL_SIZE],
            )
            .expect("stored header should be OK")
            .into_view();

            let output_index = out_point.index().unpack();
            let tx = tx.into_view();
            if let Some(cell_output) = tx.outputs().get(output_index) {
                let output_data = tx
                    .outputs_data()
                    .get(output_index)
                    .expect("output_data's index should be same as output")
                    .raw_data();
                let output_data_data_hash = CellOutput::calc_data_hash(&output_data);
                let cell_meta = CellMeta {
                    out_point: out_point.clone(),
                    cell_output,
                    transaction_info: Some(TransactionInfo {
                        block_hash,
                        block_epoch: header.epoch(),
                        block_number,
                        index: tx_index as usize,
                    }),
                    data_bytes: output_data.len() as u64,
                    mem_cell_data: Some(output_data),
                    mem_cell_data_hash: Some(output_data_data_hash),
                };
                return CellStatus::Live(cell_meta);
            }
        }
        CellStatus::Unknown
    }

    pub async fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.get(Key::BlockHash(hash).into_vec())
            .await
            .map(|v| {
                v.map(|v| {
                    Header::from_slice(&v[..Header::TOTAL_SIZE])
                        .expect("stored Header")
                        .into_view()
                })
            })
            .expect("db get should be ok")
    }
}

async fn open_iterator(
    store: &ObjectStore,
    start_key_bound: &[u8],
    order: CursorDirection,
) -> Result<ManagedCursor> {
    let index = store.index("key").unwrap();

    Ok(index
        .open_cursor(
            Some(
                KeyRange::lower_bound(
                    &serde_wasm_bindgen::to_value(&start_key_bound).unwrap(),
                    Some(false),
                )
                .unwrap()
                .into(),
            ),
            Some(order),
        )?
        .await?
        .unwrap()
        .into_managed())
}

pub async fn collect_iterator<F>(
    store: &ObjectStore,
    start_key_bound: &[u8],
    order: CursorDirection,
    take_while: F,
    limit: usize,
    skip: usize,
) -> Result<Vec<KV>>
where
    F: Fn(&[u8]) -> bool,
{
    let mut iter = open_iterator(store, start_key_bound, order).await?;

    let mut res = Vec::new();

    let mut skip_index = 0;

    if iter.key().is_err() {
        return Ok(res);
    }

    let raw_kv = serde_wasm_bindgen::from_value::<KV>(iter.value()?.unwrap()).unwrap();

    if take_while(&raw_kv.key) {
        skip_index += 1;
        if skip_index > skip {
            res.push(raw_kv);
        }
    } else {
        return Ok(res);
    }

    while iter.next(None).await.is_ok() {
        if iter.key().is_err() {
            return Ok(res);
        }

        if res.len() >= limit {
            return Ok(res);
        }

        let key = iter.key().unwrap();
        if key.is_none() {
            return Ok(res);
        }

        let raw_kv = serde_wasm_bindgen::from_value::<KV>(iter.value()?.unwrap()).unwrap();
        if take_while(&raw_kv.key) {
            skip_index += 1;
            if skip_index > skip {
                res.push(raw_kv);
            }
        } else {
            return Ok(res);
        }
    }

    Ok(res)
}

async fn collect_iterator_keys<F>(
    store: &ObjectStore,
    start_key_bound: &[u8],
    order: CursorDirection,
    take_while: F,
    limit: usize,
    skip: usize,
) -> Result<Vec<Vec<u8>>>
where
    F: Fn(&[u8]) -> bool,
{
    let mut iter = open_iterator(store, start_key_bound, order).await?;

    let mut res = Vec::new();
    let mut skip_index = 0;

    if iter.key().is_err() {
        return Ok(res);
    }

    let raw_key = serde_wasm_bindgen::from_value::<Vec<u8>>(iter.key()?.unwrap()).unwrap();

    if take_while(&raw_key) {
        skip_index += 1;
        if skip_index > skip {
            res.push(raw_key);
        }
    } else {
        return Ok(res);
    }

    while iter.next(None).await.is_ok() {
        if iter.key().is_err() {
            return Ok(res);
        }

        if res.len() >= limit {
            return Ok(res);
        }

        let key = iter.key().unwrap();
        if key.is_none() {
            return Ok(res);
        }

        let raw_key = serde_wasm_bindgen::from_value::<Vec<u8>>(key.unwrap()).unwrap();
        if take_while(&raw_key) {
            skip_index += 1;
            if skip_index > skip {
                res.push(raw_key);
            }
        } else {
            return Ok(res);
        }
    }

    Ok(res)
}
