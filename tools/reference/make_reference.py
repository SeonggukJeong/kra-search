"""원본(HF) EmbeddingGemma 2 텍스트 임베딩을 만들어 GGUF 출력과 비교할 기준값을 저장한다. 개발 PC 전용."""
import json
import sys

from sentence_transformers import SentenceTransformer

TEXTS = [
    "task: search result | query: 경마 매출 감소 원인",
    "task: search result | query: 작년 예산 관련 공문",
    "task: search result | query: 기재부 협조 요청",
    "title: 2025년 마권 발매 실적 보고 | text: 온라인 발매 전환 지연과 입장객 감소가 주요 원인으로 분석된다.",
    "title: none | text: 내일은 전국이 맑고 기온이 오르겠다.",
    "title: 행사 협조 요청 | text: 붙임 1. 행사 계획 1부. 끝.",
    "title: 규정 | text: 제1조(목적) 이 규정은 문서의 관리에 필요한 사항을 정함을 목적으로 한다.",
    "title: none | text: The quarterly revenue decreased due to lower attendance.",
]


def main(out: str) -> None:
    # 텍스트 전용 로드(모델 카드 "Selective Encoder Loading")
    model = SentenceTransformer(
        "google/embeddinggemma-2", config_kwargs={"vision_config": None, "audio_config": None}
    )
    embs = model.encode(TEXTS, prompt_name=None, normalize_embeddings=False, convert_to_numpy=True)
    with open(out, "w", encoding="utf-8") as f:
        json.dump(
            {
                "model": "google/embeddinggemma-2",
                "items": [{"text": t, "embedding": [float(x) for x in e[:768]]} for t, e in zip(TEXTS, embs)],
            },
            f,
            ensure_ascii=False,
        )


if __name__ == "__main__":
    main(sys.argv[1])
