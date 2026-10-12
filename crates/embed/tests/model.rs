use embed::engine::{EmbedError, Embedder, EngineOptions};
use embed::profile::ModelProfile;
use embed::vector::{cosine, truncate_normalize};
use std::path::PathBuf;

fn model() -> PathBuf {
    std::env::var("KRA_MODEL").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/embeddinggemma-2-Q8_0.gguf"))
    })
}
fn opts() -> EngineOptions {
    EngineOptions { threads: 4, max_tokens: 1024, backend_dir: None }
}

#[test]
fn missing_model_is_clear_error() {
    let e = Embedder::load(std::path::Path::new("/no/such/model.gguf"), &opts()).err().unwrap();
    assert!(matches!(e, EmbedError::ModelNotFound(_)), "{e:?}");
}

#[test]
#[ignore = "needs model file"]
fn embeds_with_native_dim_and_is_deterministic() {
    let mut m = Embedder::load(&model(), &opts()).unwrap();
    assert_eq!(m.n_embd(), 768);
    let p = ModelProfile::embeddinggemma2();
    let a = m.embed(&[p.render_query("경마 매출 감소 원인")]).unwrap();
    let b = m.embed(&[p.render_query("경마 매출 감소 원인")]).unwrap();
    assert_eq!(a[0].vector.len(), 768);
    assert!(cosine(&a[0].vector, &b[0].vector) > 0.9999);
}

#[test]
#[ignore = "needs model file"]
fn batch_equals_single() {
    let mut m = Embedder::load(&model(), &opts()).unwrap();
    let p = ModelProfile::embeddinggemma2();
    let xs = vec![p.render_doc(Some("보고서"), "마권 발매액이 전년 대비 감소하였다."), p.render_doc(None, "내일 날씨는 맑음.")];
    let batch = m.embed(&xs).unwrap();
    for (i, x) in xs.iter().enumerate() {
        let single = m.embed(std::slice::from_ref(x)).unwrap();
        assert!(cosine(&batch[i].vector, &single[0].vector) > 0.999);
    }
}

#[test]
#[ignore = "needs model file"]
fn korean_semantic_sanity() {
    let mut m = Embedder::load(&model(), &opts()).unwrap();
    let p = ModelProfile::embeddinggemma2();
    let v = m
        .embed(&[
            p.render_query("경마 매출 감소"),
            p.render_doc(Some("발매 실적"), "마권 발매액이 전년 대비 12% 줄었다."),
            p.render_doc(Some("기상"), "내일은 전국이 맑고 기온이 오르겠다."),
        ])
        .unwrap();
    let n = |i: usize| truncate_normalize(&v[i].vector, 768).unwrap();
    assert!(cosine(&n(0), &n(1)) > cosine(&n(0), &n(2)));
}

#[test]
#[ignore = "needs model file"]
fn rejects_empty_input() {
    let mut m = Embedder::load(&model(), &opts()).unwrap();
    assert!(matches!(m.embed(&["   ".to_string()]), Err(EmbedError::EmptyInput)));
}

#[test]
#[ignore = "needs model file"]
fn embeds_overlong_input_truncated() {
    let mut m = Embedder::load(&model(), &opts()).unwrap();
    let long = "공공기관 업무 보고서 문장입니다. ".repeat(3000);
    let r = m.embed(&[long]).unwrap();
    assert!(r[0].truncated);
    assert_eq!(r[0].n_tokens, 1024);
    assert!(truncate_normalize(&r[0].vector, 768).is_some());
}
