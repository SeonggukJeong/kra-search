//! kra-search 앱(M1: 명령줄). 색인의 유일한 쓰기 주체(스펙 §5.1). 화면(Tauri)은 M3.
mod pipeline;
mod search;
mod store;

pub use pipeline::{flush, run_index, IndexError, IndexSummary};
pub use search::{fail_label, search, Hit};
pub use store::{lock_index, open_store, open_store_readonly, reset_store, Store};
