use std::cell::RefCell;
use std::str::FromStr;

use db::{handle_db_command, open_database};
use idb::Database;
use light_client_db_common::{read_command_payload, write_command_with_payload};
use light_client_db_common::{InputCommand, OutputCommand};
use log::{debug, info};
use util::{wait_for_command, wait_for_command_sync};
use wasm_bindgen::{prelude::wasm_bindgen, JsCast, JsValue};
use web_sys::js_sys::{Int32Array, SharedArrayBuffer, Uint8Array};

mod db;
mod util;
thread_local! {
    static INPUT_BUFFER: RefCell<Option<SharedArrayBuffer>> = const { RefCell::new(None) };
    static OUTPUT_BUFFER: RefCell<Option<SharedArrayBuffer>> = const { RefCell::new(None) };
}
#[wasm_bindgen]
/// Set `SharedArrayBuffer` used for communicating with light client worker. This must be called before executing `main_loop`
/// input - The buffer used for sending data from light client worker to db worker
/// output - The buffer used for sending data from db worker to light client worker
pub fn set_shared_array(input: JsValue, output: JsValue) {
    console_error_panic_hook::set_once();
    INPUT_BUFFER.with(|v| {
        *v.borrow_mut() = Some(
            input
                .dyn_into()
                .expect("input buffer must be a SharedArrayBuffer"),
        );
    });
    OUTPUT_BUFFER.with(|v| {
        *v.borrow_mut() = Some(
            output
                .dyn_into()
                .expect("output buffer must be a SharedArrayBuffer"),
        );
    });
}

#[wasm_bindgen]
/// Enter the main loop of db worker. Once entered, db worker will read commands from input buffer (previously set by set_shared_array), handle it, and write response to output buffer.
/// log_level - Level of logs, such as `debug`, `info`.
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
