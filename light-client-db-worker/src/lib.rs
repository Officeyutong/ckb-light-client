use std::cell::RefCell;
use std::str::FromStr;

use anyhow::{anyhow, Context};
use idb::{
    CursorDirection, Database, DatabaseEvent, Factory, IndexParams, KeyPath, KeyRange,
    ManagedCursor, ObjectStore, ObjectStoreParams, TransactionMode, TransactionResult,
};
use light_client_db_common::{
    ckb_cursor_direction_to_idb, read_command_payload, write_command_with_payload,
};
use light_client_db_common::{
    DbCommandRequest, DbCommandResponse, InputCommand, OutputCommand, KV,
};
use log::{debug, info};
use serde::Deserialize;
use wasm_bindgen::{prelude::wasm_bindgen, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::js_sys::{Atomics, Int32Array, Promise, SharedArrayBuffer, Uint8Array};
async fn open_iterator(
    store: &ObjectStore,
    start_key_bound: &[u8],
    order: CursorDirection,
) -> Result<ManagedCursor, idb::Error> {
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
) -> anyhow::Result<Vec<KV>>
where
    F: Fn(&[u8]) -> bool,
{
    let mut iter = open_iterator(store, start_key_bound, order)
        .await
        .map_err(|e| anyhow!("Failed to open iterator: {e:?}"))?;

    let mut res = Vec::new();

    let mut skip_index = 0;

    if iter.key().is_err() {
        return Ok(res);
    }

    let raw_kv = serde_wasm_bindgen::from_value::<KV>(
        iter.value()
            .map_err(|e| anyhow!("Failed to read value from cursor: {e:?}"))?
            .unwrap(),
    )
    .unwrap();

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

        let raw_kv = serde_wasm_bindgen::from_value::<KV>(
            iter.value()
                .map_err(|e| anyhow!("Failed to read value from cursor: {e:?}"))?
                .unwrap(),
        )
        .unwrap();
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
) -> anyhow::Result<Vec<Vec<u8>>>
where
    F: Fn(&[u8]) -> bool,
{
    let mut iter = open_iterator(store, start_key_bound, order)
        .await
        .map_err(|e| anyhow!("Failed to open iterator: {e:?}"))?;

    let mut res = Vec::new();
    let mut skip_index = 0;

    if iter.key().is_err() {
        return Ok(res);
    }

    let raw_key = serde_wasm_bindgen::from_value::<Vec<u8>>(
        iter.key()
            .map_err(|e| anyhow!("Failed to read value from cursor: {e:?}"))?
            .unwrap(),
    )
    .unwrap();

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
thread_local! {
    static INPUT_BUFFER: RefCell<Option<SharedArrayBuffer>> = const { RefCell::new(None) };
    static OUTPUT_BUFFER: RefCell<Option<SharedArrayBuffer>> = const { RefCell::new(None) };
}
#[wasm_bindgen]
pub fn set_shared_array(input: JsValue, output: JsValue) {
    console_error_panic_hook::set_once();
    INPUT_BUFFER.with(|v| {
        *v.borrow_mut() = Some(input.dyn_into().unwrap());
    });
    OUTPUT_BUFFER.with(|v| {
        *v.borrow_mut() = Some(output.dyn_into().unwrap());
    });
}

async fn handle_db_command<F>(
    db: &Database,
    store_name: &str,
    cmd: DbCommandRequest,
    invoke_take_while: F,
) -> anyhow::Result<DbCommandResponse>
where
    F: Fn(&[u8]) -> bool,
{
    debug!("Handle command: {:?}", cmd);
    let tx_mode = match cmd {
        DbCommandRequest::Iterator { .. } | DbCommandRequest::IteratorKey { .. } => {
            TransactionMode::ReadOnly
        }
        DbCommandRequest::Read { .. } => TransactionMode::ReadOnly,
        DbCommandRequest::Put { .. }
        | DbCommandRequest::Delete { .. }
        | DbCommandRequest::TakeWhileBenchmark { .. } => TransactionMode::ReadWrite,
    };
    let tran = db
        .transaction(&[&store_name], tx_mode)
        .map_err(|e| anyhow!("Failed to create transaction: {:?}", e))?;

    let store = tran
        .object_store(&store_name)
        .map_err(|e| anyhow!("Unable to find store {}: {}", store_name, e))?;

    let result = match cmd {
        DbCommandRequest::Read { keys } => {
            let mut res = Vec::new();
            for key in keys {
                let key = serde_wasm_bindgen::to_value(&key)
                    .map_err(|e| anyhow!("Unable to convert key to JsValue: {:?}", e))?;

                res.push(
                    store
                        .get(key)
                        .map_err(|e| anyhow!("Failed to send get request: {:?}", e))?
                        .await
                        .map_err(|e| anyhow!("Failed to fetch value: {:?}", e))?
                        .map(|v| serde_wasm_bindgen::from_value::<KV>(v).unwrap().value),
                );
            }
            DbCommandResponse::Read { values: res }
        }
        DbCommandRequest::Put { kvs } => {
            debug!("Putting: {:?}", kvs);
            for kv in kvs {
                let key = serde_wasm_bindgen::to_value(&kv.key).unwrap();
                let value = serde_wasm_bindgen::to_value(&kv).unwrap();
                store
                    .put(&value, Some(&key))
                    .map_err(|e| anyhow!("Failed to send put request: {:?}", e))?
                    .await
                    .map_err(|e| anyhow!("Failed to put: {:?}", e))?;
            }
            DbCommandResponse::Put
        }
        DbCommandRequest::Delete { keys } => {
            for key in keys {
                let key = serde_wasm_bindgen::to_value(&key).unwrap();
                store
                    .delete(key)
                    .map_err(|e| anyhow!("Failed to send delete request: {:?}", e))?
                    .await
                    .map_err(|e| anyhow!("Failed to delete: {:?}", e))?;
            }
            DbCommandResponse::Delete
        }

        DbCommandRequest::Iterator {
            start_key_bound,
            order,
            limit,
            skip,
        } => {
            let kvs = collect_iterator(
                &store,
                &start_key_bound,
                ckb_cursor_direction_to_idb(order),
                invoke_take_while,
                limit,
                skip,
            )
            .await
            .with_context(|| anyhow!("Failed to collect iterator"))?;
            debug!("At {}", line!());
            DbCommandResponse::Iterator { kvs }
        }
        DbCommandRequest::IteratorKey {
            start_key_bound,
            order,
            limit,
            skip,
        } => {
            let keys = collect_iterator_keys(
                &store,
                &start_key_bound,
                ckb_cursor_direction_to_idb(order),
                invoke_take_while,
                limit,
                skip,
            )
            .await
            .with_context(|| anyhow!("Failed to collect iterator keys"))?;
            DbCommandResponse::IteratorKey { keys }
        }
        DbCommandRequest::TakeWhileBenchmark { test_count } => {
            let start = web_time::Instant::now();
            for _ in 0..test_count {
                invoke_take_while(&vec![1u8; 64]);
            }
            DbCommandResponse::TakeWhileBenchmark {
                duration_in_ns: start.elapsed().as_nanos() as usize,
            }
        }
    };
    assert_eq!(TransactionResult::Committed, tran.await.unwrap());
    debug!("Command result={:?}", result);
    Ok(result)
}

#[derive(Deserialize)]
struct WaitAsyncResponse {
    #[serde(with = "serde_wasm_bindgen::preserve")]
    value: JsValue,
    #[serde(rename = "async")]
    async_: bool,
}

fn wait_for_command_sync(
    state_arr: &Int32Array,
    holding_command: InputCommand,
) -> anyhow::Result<InputCommand> {
    Atomics::wait(state_arr, 0, holding_command as i32)
        .map_err(|e| anyhow!("Failed to call wait async: {:?}", e))?;

    InputCommand::try_from(state_arr.get_index(0)).with_context(|| anyhow!("Bad command"))
}

async fn wait_for_command(
    state_arr: &Int32Array,
    holding_command: InputCommand,
) -> anyhow::Result<InputCommand> {
    let obj = Atomics::wait_async(state_arr, 0, holding_command as i32)
        .map_err(|e| anyhow!("Failed to call wait async: {:?}", e))?;
    let resp = serde_wasm_bindgen::from_value::<WaitAsyncResponse>((&obj).into())
        .map_err(|e| anyhow!("Failed to deserialize result of wait async: {:?}", e))?;

    if resp.async_ {
        JsFuture::from(
            resp.value
                .dyn_into::<Promise>()
                .expect("Value must be casted into promise!"),
        )
        .await
        .expect("value must not fail!");
    } else {
        let value = resp
            .value
            .as_string()
            .expect("Value must be casted to string");
        if value != "not-equal" {
            panic!("Value must be not-equal if should_wait_async is false");
        }
    }

    InputCommand::try_from(state_arr.get_index(0)).with_context(|| anyhow!("Bad command"))
}

async fn open_database(store_name: impl AsRef<str>) -> anyhow::Result<Database> {
    let factory = Factory::new().map_err(|e| anyhow!("Failed to create db factory: {:?}", e))?;
    let mut open_request = factory
        .open("ckb-light-client", Some(1))
        .map_err(|e| anyhow!("Failed to send db open request: {:?}", e))?;
    let store_name = store_name.as_ref().to_owned();
    let new_store_name = store_name.clone();
    open_request.on_upgrade_needed(move |event| {
        let database = event.database().unwrap();
        let store_params = ObjectStoreParams::new();

        let store = database
            .create_object_store(&new_store_name, store_params)
            .unwrap();
        let mut index_params = IndexParams::new();
        index_params.unique(true);
        store
            .create_index("key", KeyPath::new_single("key"), Some(index_params))
            .unwrap();
    });
    open_request
        .await
        .map_err(|e| anyhow!("Failed to open database: {:?}", e))
}

#[wasm_bindgen]
pub async fn main_loop(log_level: &str) {
    wasm_logger::init(wasm_logger::Config::new(
        log::Level::from_str(log_level).expect("Invalid log level"),
    ));

    let (input_i32_arr, input_u8_arr) = INPUT_BUFFER.with(|x| {
        let binding = x.borrow();
        let buf = binding.as_ref().unwrap();
        (Int32Array::new(buf), Uint8Array::new(buf))
    });
    let (output_i32_arr, output_u8_arr) = OUTPUT_BUFFER.with(|x| {
        let binding = x.borrow();
        let buf = binding.as_ref().unwrap();
        (Int32Array::new(buf), Uint8Array::new(buf))
    });

    let mut db: Option<(Database, String)> = None;

    loop {
        let cmd = wait_for_command(&input_i32_arr, InputCommand::Waiting)
            .await
            .expect("Unable to wait for command");
        // Clean it to avoid infinite loop
        input_i32_arr.set_index(0, InputCommand::Waiting as i32);
        match cmd {
            InputCommand::OpenDatabase => {
                let store_name =
                    read_command_payload::<String>(&input_i32_arr, &input_u8_arr).unwrap();
                match open_database(&store_name).await {
                    Ok(o) => {
                        db = Some((o, store_name));
                        write_command_with_payload(
                            OutputCommand::OpenDatabaseResponse as i32,
                            true,
                            &output_i32_arr,
                            &output_u8_arr,
                        )
                        .unwrap();
                    }
                    Err(err) => write_command_with_payload(
                        OutputCommand::Error as i32,
                        format!("{:?}", err),
                        &output_i32_arr,
                        &output_u8_arr,
                    )
                    .unwrap(),
                }
            }
            InputCommand::DbRequest => {
                let db_cmd = read_command_payload(&input_i32_arr, &input_u8_arr).unwrap();
                let (db, store_name) = db.as_ref().expect("Database not opened yet");
                let result = handle_db_command(db, store_name, db_cmd, |buf| {
                    input_i32_arr.set_index(0, InputCommand::Waiting as i32);
                    debug!("Invoking request take while with args {:?}", buf);
                    write_command_with_payload(
                        OutputCommand::RequestTakeWhile as i32,
                        buf,
                        &output_i32_arr,
                        &output_u8_arr,
                    )
                    .unwrap();
                    // Sync wait here, so transaction of IndexedDB won't be commited (it will be commited once control flow was returned from sync call stack)
                    wait_for_command_sync(&input_i32_arr, InputCommand::Waiting).unwrap();

                    let result =
                        read_command_payload::<bool>(&input_i32_arr, &input_u8_arr).unwrap();
                    debug!("Received take while result {}", result);
                    input_i32_arr.set_index(0, InputCommand::Waiting as i32);
                    result
                })
                .await;
                debug!("db command result: {:?}", result);
                match result {
                    Ok(o) => write_command_with_payload(
                        OutputCommand::DbResponse as i32,
                        &o,
                        &output_i32_arr,
                        &output_u8_arr,
                    )
                    .unwrap(),
                    Err(e) => write_command_with_payload(
                        OutputCommand::Error as i32,
                        format!("{:?}", e),
                        &output_i32_arr,
                        &output_u8_arr,
                    )
                    .unwrap(),
                };
            }
            InputCommand::Shutdown => break,
            InputCommand::Waiting | InputCommand::ResponseTakeWhile => unreachable!(),
        }
    }
    info!("Db worker main loop exited");
}
