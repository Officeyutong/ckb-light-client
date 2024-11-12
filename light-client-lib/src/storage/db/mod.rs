#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{Batch, Storage};

#[cfg(target_arch = "wasm32")]
mod browser_test;
#[cfg(target_arch = "wasm32")]
pub use browser_test::{Batch, Direction, Storage};
// #[cfg(target_arch = "wasm32")]
// mod browser;
// #[cfg(target_arch = "wasm32")]
// pub use browser::{collect_iterator, Batch, CursorDirection, Storage};
