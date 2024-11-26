use anyhow::Context;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use web_sys::js_sys::{Atomics, Int32Array, Uint8Array};
#[derive(Serialize, Deserialize, Debug)]
pub struct KV {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}
#[derive(Serialize, Deserialize, Default, Debug)]
pub enum CursorDirection {
    #[default]
    Next,
    NextUnique,
    Prev,
    PrevUnique,
}
#[derive(Serialize, Deserialize, Debug)]
pub enum DbCommandResponse {
    Read { values: Vec<Option<Vec<u8>>> },
    Put,
    Delete,
    Iterator { kvs: Vec<KV> },
    IteratorKey { keys: Vec<Vec<u8>> },
    TakeWhileBenchmark { duration_in_ns: usize },
}

#[derive(Deserialize, Serialize, Debug)]
pub enum DbCommandRequest {
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
        limit: usize,
        skip: usize,
    },
    IteratorKey {
        start_key_bound: Vec<u8>,
        order: CursorDirection,
        limit: usize,
        skip: usize,
    },
    TakeWhileBenchmark {
        test_count: usize,
    },
}
#[repr(i32)]
pub enum InputCommand {
    Waiting = 0,
    OpenDatabase = 1,
    DbRequest = 2,
    Shutdown = 3,
    ResponseTakeWhile = 20,
}

impl TryFrom<i32> for InputCommand {
    type Error = anyhow::Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Waiting),
            1 => Ok(Self::OpenDatabase),
            2 => Ok(Self::DbRequest),
            3 => Ok(Self::Shutdown),
            20 => Ok(Self::ResponseTakeWhile),
            s => Err(anyhow!("Invalid command: {}", s)),
        }
    }
}

#[repr(i32)]
pub enum OutputCommand {
    Waiting = 0,
    OpenDatabaseResponse = 1,
    DbResponse = 2,
    ShutdownResponse = 3,
    Error = 10,
    RequestTakeWhile = 20,
}

impl TryFrom<i32> for OutputCommand {
    type Error = anyhow::Error;

    fn try_from(value: i32) -> Result<Self, <OutputCommand as TryFrom<i32>>::Error> {
        match value {
            0 => Ok(Self::Waiting),
            1 => Ok(Self::OpenDatabaseResponse),
            2 => Ok(Self::DbResponse),
            3 => Ok(Self::ShutdownResponse),
            10 => Ok(Self::Error),
            20 => Ok(Self::RequestTakeWhile),
            s => Err(anyhow!("Invalid command: {}", s)),
        }
    }
}

pub fn ckb_cursor_direction_to_idb(x: crate::CursorDirection) -> idb::CursorDirection {
    use crate::CursorDirection;
    match x {
        CursorDirection::Next => idb::CursorDirection::Next,
        CursorDirection::NextUnique => idb::CursorDirection::NextUnique,
        CursorDirection::Prev => idb::CursorDirection::Prev,
        CursorDirection::PrevUnique => idb::CursorDirection::PrevUnique,
    }
}
pub fn idb_cursor_direction_to_ckb(x: idb::CursorDirection) -> crate::CursorDirection {
    use crate::CursorDirection;

    match x {
        idb::CursorDirection::Next => CursorDirection::Next,
        idb::CursorDirection::NextUnique => CursorDirection::NextUnique,
        idb::CursorDirection::Prev => CursorDirection::Prev,
        idb::CursorDirection::PrevUnique => CursorDirection::PrevUnique,
    }
}
use anyhow::anyhow;

pub fn write_command_with_payload<T: Serialize>(
    cmd: i32,
    data: T,
    i32arr: &Int32Array,
    u8arr: &Uint8Array,
) -> anyhow::Result<()> {
    let result_buf = bincode::serialize(&data)
        .with_context(|| anyhow!("Failed to serialize command payload"))?;

    i32arr.set_index(1, result_buf.len() as i32);
    u8arr
        .subarray(8, 8 + result_buf.len() as u32)
        .copy_from(&result_buf);
    i32arr.set_index(0, cmd);
    Atomics::notify(i32arr, 0).map_err(|e| anyhow!("Failed to notify: {e:?}"))?;
    Ok(())
}
pub fn read_command_payload<T: DeserializeOwned>(
    i32arr: &Int32Array,
    u8arr: &Uint8Array,
) -> anyhow::Result<T> {
    let length = i32arr.get_index(1) as u32;
    let mut buf = vec![0u8; length as usize];
    u8arr.subarray(8, 8 + length).copy_to(&mut buf);

    let result =
        bincode::deserialize::<T>(&buf).with_context(|| anyhow!("Failed to decode command"))?;
    Ok(result)
}
