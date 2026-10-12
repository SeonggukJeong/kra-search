# M0 모델·llama.cpp 고정 기록

## llama.cpp
- 서브모듈: `crates/llama-sys/vendor/llama.cpp`
- 고정 태그: **b11574** (EmbeddingGemma 2 지원 커밋 `4fbc76dec` "model: support embeddinggemma2 (#30054)" 포함 확인)
- `src/llama-arch.cpp`: `{ LLM_ARCH_GEMMA_EMBEDDING2, "gemma-embedding2" }`
- 빌드 시 추가로 끈 옵션: `LLAMA_BUILD_APP=OFF`, `LLAMA_BUILD_COMMON=OFF` (b11574의 통합 실행 파일 `app/`이 common 헤더를 요구해 실패)

## llama.h API 확인 (b11574)
계획서 Task 5의 이름이 모두 존재: `llama_backend_init`, `llama_model_load_from_file`, `llama_model_free`,
`llama_init_from_model`, `llama_free`, `llama_get_memory`, `llama_memory_clear`, `llama_model_get_vocab`,
`llama_model_n_embd`, `llama_model_has_encoder`, `llama_model_has_decoder`, `llama_batch_init`, `llama_encode`,
`llama_get_embeddings_seq`, `llama_tokenize`, `llama_print_system_info`, `LLAMA_POOLING_TYPE_MEAN`,
`ggml_backend_load_all_from_path`. `llama_batch.logits` 필드명 유지.

## 개발용 GGUF (unsloth/embeddinggemma-2-GGUF)
| 로컬 파일 | 원본 파일 | 크기 | SHA-256 |
|---|---|---|---|
| `models/embeddinggemma-2-Q8_0.gguf` | `embeddinggemma-2-Q8_0.gguf` | 309,855,520 | `6f1bd4ac6c5df7444f9cca7ca36cafe6cfa34cd6f49fefb1e0b4be8143aed8bc` |
| `models/embeddinggemma-2-Q4.gguf` | `embeddinggemma-2-UD-Q4_K_XL.gguf` | 175,673,856 | `ea905fd08e8061db77a0031cbb5096d459d7cfd2bdd0ad2ba66e483f16719493` |

`tools/fetch-models.sh`로 받는다(텍스트 GGUF만, `mmproj` 제외).
