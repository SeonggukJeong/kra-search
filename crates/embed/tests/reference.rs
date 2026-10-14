use embed::engine::{Embedder, EngineOptions};
use embed::vector::cosine;

#[derive(serde::Deserialize)]
struct Item {
    text: String,
    embedding: Vec<f32>,
}
#[derive(serde::Deserialize)]
struct Fixture {
    items: Vec<Item>,
}

fn check(model_file: &str, min_mean: f32) {
    let f: Fixture = serde_json::from_str(include_str!("fixtures/reference.json")).unwrap();
    let path = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/")).join(model_file);
    let mut m = Embedder::load(&path, &EngineOptions { threads: 4, max_tokens: 1024, backend_dir: None }).unwrap();
    let texts: Vec<String> = f.items.iter().map(|i| i.text.clone()).collect();
    let got = m.embed(&texts).unwrap();
    let sims: Vec<f32> = got.iter().zip(&f.items).map(|(g, r)| cosine(&g.vector, &r.embedding)).collect();
    let mean = sims.iter().sum::<f32>() / sims.len() as f32;
    eprintln!("{model_file}: mean={mean:.4} min={:.4} all={sims:?}", sims.iter().cloned().fold(1.0, f32::min));
    assert!(mean >= min_mean, "{model_file} mean cosine {mean} < {min_mean}");
}

#[test]
#[ignore = "needs model file"]
fn q8_matches_reference() {
    check("embeddinggemma-2-Q8_0.gguf", 0.99);
}

#[test]
#[ignore = "needs model file"]
fn q4_matches_reference() {
    check("embeddinggemma-2-Q4.gguf", 0.97);
}
