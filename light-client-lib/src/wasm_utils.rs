use std::{cell::RefCell, sync::atomic::AtomicBool};

use light_client_db_common::write_command_with_payload_json;
use serde::Serialize;
use wasm_bindgen::{prelude::wasm_bindgen, JsCast, JsValue};
use web_sys::js_sys::{Atomics, Int32Array, SharedArrayBuffer, Uint8Array};

static TRACE_CALLBACK_ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TRACE_LOG_BUFFER: RefCell<Option<SharedArrayBuffer>> = RefCell::new(None);
}

/**
 * Set a shared array buffer used to receive logs
 */
#[wasm_bindgen]
pub fn set_trace_shared_array_buffer(buf: JsValue) {
    TRACE_LOG_BUFFER.with_borrow_mut(move |val| {
        *val = Some(buf.dyn_into().expect("Must provide a SharedArrayBuffer"));
    })
}

#[wasm_bindgen]
pub fn enable_trace_record_callback() {
    TRACE_CALLBACK_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum TraceRecord {
    FinalizeCheckPoints {
        count: usize,
        stop_at: u32,
    },
    DownloadBlock {
        start_at: u64,
        count: u64,
        matched_count: usize,
    },
}

pub fn send_trace_record(record: &TraceRecord) {
    if TRACE_CALLBACK_ENABLED.load(std::sync::atomic::Ordering::SeqCst) {
        let (u8buf, i32buf) = TRACE_LOG_BUFFER.with_borrow(|v| {
            let buf = v.as_ref().unwrap();
            (Uint8Array::new(buf), Int32Array::new(buf))
        });
        write_command_with_payload_json(1, record, &i32buf, &u8buf).unwrap();
        Atomics::notify(&i32buf, 0).unwrap();
        Atomics::wait_with_timeout(&i32buf, 0, 1, 100.0).unwrap();
    }
}
