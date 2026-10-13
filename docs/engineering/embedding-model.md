# 임베딩 모델 사실 정리 (EmbeddingGemma 2)

모델을 다루는 코드(`crates/embed`)나 사용자 교체 모델(D6)을 만들 때 틀리기 쉬운 사실들. 근거는 M0 측정([docs/m0](../m0/)).

| 항목 | 값 | 틀리면 | 근거 |
|---|---|---|---|
| 출력 차원 | **768** — `llama_model_n_embd_out`. 내부 hidden 은 512(`llama_model_n_embd`) | 512차원 벡터 저장 | INC-002 |
| 풀링 | mean (`LLAMA_POOLING_TYPE_MEAN`) | 품질 저하 | 모델 카드 |
| 축소 | MRL: 앞에서 자른 뒤 **직접 L2 정규화**(`vector::truncate_normalize`). 저장 차원 512 잠정 | 코사인 비교 오류 | docs/m0/results.md |
| 질의 접두어 | `task: search result \| query: {질의}` | 검색 품질 저하 | `profile.rs` |
| 문서 접두어 | `title: {제목 또는 none} \| text: {본문}` | 〃 | `profile.rs` |
| 최대 토큰 | 1,024(프로필) | 긴 입력 잘림 표시(`truncated`) | `profile.rs` |
| 호출 | 인코더 전용 → `llama_encode` 먼저 | 경고 폭주 | INC-007 |
| 어휘 | 262k → logits 버퍼가 거대함 | 메모리 +1GB | INC-004 |
| 원본 정합성 | 원본(sentence-transformers) 대비 평균 코사인 Q8 0.9998, Q4 0.9930 | — | `embed/tests/reference.rs` |
| 토큰 밀도 | 한글 1,000자당 중앙값 647, p90 703 → A4 1쪽 ≈ 청크 2개 | 청크 수·시간 추정 오류 | eval-dev.md |
| 토큰 세기 | 부모 프로세스는 `Tokenizer`(vocab_only)로만 — 전체 모델 로드 금지 | 메모리 낭비 | INC-012 |
| GGUF 출처 | unsloth/embeddinggemma-2-GGUF: Q8_0, UD-Q4_K_XL(SHA-256 은 docs/m0/models.md) | — | KI-005 |

## 다른 모델로 바꿀 때(D6) 확인할 것
1. 출력 차원은 `n_embd_out` 으로 읽는다. 차원이 바뀌면 색인 재구축.
2. 접두어(프롬프트) 형식은 모델마다 다르다 → `ModelProfile` 에 둔다(코드에 하드코딩 금지).
3. 인코더/디코더 여부(`llama_model_has_encoder`)와 풀링 방식.
4. 어휘 크기가 크면 INC-004 패치가 그 모델 경로에도 적용되는지 메모리 시험으로 확인.
5. 원본 대비 정합성(코사인 ≥ 0.99) 참조 시험을 그 모델용으로 만든다(`tools/reference/make_reference.py`).
