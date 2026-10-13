# llama.cpp 로컬 패치

`build.rs` 가 빌드할 때 `vendor/llama.cpp` 에 순서대로 적용한다(이미 적용돼 있으면 건너뜀, 안 맞으면 빌드 실패).
`tests/patches.rs` 가 패치 파일 존재와 적용 결과를 확인하고, CI 의 `diag --test memory` 가 효과(메모리)를 확인한다.
llama.cpp 를 올릴 때는 [upgrade-llama-cpp.md](../../../docs/engineering/runbooks/upgrade-llama-cpp.md) 절차를 따른다.

| 패치 | 왜 | 효과 확인 | 없애도 되는 조건 |
|---|---|---|---|
| 0001-skip-logits-buffer-for-pooled-embeddings | 풀링 임베딩 컨텍스트도 `n_vocab × n_outputs` logits 버퍼(262k 어휘 → 약 1GB)를 잡았다 (INC-004) | 대량 임베딩 최대 메모리 1,650MB → 651MB (`diag/tests/memory.rs`) | upstream `output_reserve` 가 임베딩 전용 컨텍스트에서 logits 를 잡지 않게 바뀌었고, 패치 없이 메모리 테스트가 통과할 때 |
