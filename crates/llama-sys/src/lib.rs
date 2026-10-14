//! llama.cpp C API 바인딩(bindgen). 고정된 llama.cpp 버전은 `vendor/llama.cpp` 서브모듈 참고.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
