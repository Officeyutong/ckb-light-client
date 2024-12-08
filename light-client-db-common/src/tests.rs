use wasm_bindgen_test::wasm_bindgen_test;
use web_sys::js_sys::{Int32Array, SharedArrayBuffer, Uint8Array};

use crate::{write_command_with_payload, InputCommand, OutputCommand};

#[wasm_bindgen_test]
fn test_command_conversion() {
    assert_eq!(InputCommand::DbRequest as i32, 2);
    assert_eq!(OutputCommand::DbResponse as i32, 2);
    assert!(InputCommand::try_from(20 as i32).is_ok());
    assert!(OutputCommand::try_from(20 as i32).is_ok());
}

#[wasm_bindgen_test]
// #[test]
fn test_command_write() {
    let arr_buf = SharedArrayBuffer::new(100);
    let i32arr = Int32Array::new(&arr_buf);
    let u8arr = Uint8Array::new(&arr_buf);
    write_command_with_payload(
        InputCommand::DbRequest as i32,
        vec![1, 2, 3, 4],
        &i32arr,
        &u8arr,
    )
    .unwrap();
    assert_eq!(i32arr.get_index(0), InputCommand::DbRequest as i32);
    let mut buf = vec![0u8; 100];
    u8arr.copy_to(&mut buf);
    let result =
        bincode::deserialize::<Vec<i32>>(&buf[8..i32arr.get_index(1) as usize + 8]).unwrap();
    assert_eq!(result, vec![1, 2, 3, 4]);
}
