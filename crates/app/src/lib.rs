//! kra-search 앱(M1: 명령줄). 색인의 유일한 쓰기 주체(스펙 §5.1). 화면(Tauri)은 M3.
pub mod embedding;
pub mod model;
mod pipeline;
mod search;
mod store;
pub mod vectors;

// 워커가 내보내는 글자 상한(extract)과 키워드 본문 상한(index)은 같은 값이어야 한다(DEC-036) — 다르면 컴파일되지 않는다.
const _: () = assert!(extract::MAX_DOC_CHARS == index::keyword::BODY_MAX_CHARS);

pub use pipeline::{flush, run_index, IndexError, IndexSummary};
pub use search::{fail_label, search, Hit};
pub use store::{lock_index, open_store, open_store_readonly, reset_store, Store};
