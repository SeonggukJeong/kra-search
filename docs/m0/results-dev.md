# M0 개발 PC 측정 결과

측정 PC: Apple Silicon(arm64) macOS · llama.cpp b11574 (CPU, Metal 끔)

## 1. GGUF ↔ 원본 모델 출력 일치 (Task 6)
원본: HF `google/embeddinggemma-2`(Apache-2.0, 비게이트) sentence-transformers, 텍스트 전용(`config_kwargs={"vision_config": None, "audio_config": None}`), 정규화 전 768차원. 입력 8개(한국어 질의 3·문서 4·영어 문서 1).

| GGUF | 평균 코사인 | 최소 | 기준 | 판정 |
|---|---|---|---|---|
| Q8_0 | **0.9998** | 0.9995 | ≥ 0.99 | 통과 |
| UD-Q4_K_XL | **0.9930** | 0.9859 | (참고) ≥ 0.97 | 통과 |

- 프롬프트 형식은 모델의 `config_sentence_transformers.json`과 일치 확인(`task: search result | query: `, `title: none | text: `).
- llama.cpp 출력 차원은 `llama_model_n_embd_out` = 768 (내부 은닉 차원 512).
