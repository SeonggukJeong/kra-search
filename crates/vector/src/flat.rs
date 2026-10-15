//! 평면(flat) 벡터 저장소: int8 정밀 스캔 + 이진 후보 선별 후 재계산.

use crate::{asym_score, dot_i8, hamming, quantize_i8, sign_bits, truncate_normalize};

pub struct FlatIndex {
    dim: usize,
    bit_dim: usize,
    words: usize,
    codes: Vec<i8>,
    inv_scales: Vec<f32>,
    bits: Vec<u64>,
}

impl FlatIndex {
    /// `dim`: int8 재계산 차원, `bit_dim`: 이진 후보 선별 차원.
    pub fn new(dim: usize, bit_dim: usize) -> Self {
        Self { dim, bit_dim, words: bit_dim.div_ceil(64), codes: vec![], inv_scales: vec![], bits: vec![] }
    }

    pub fn len(&self) -> usize {
        self.inv_scales.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 원본 차원 벡터를 추가한다. 차원 부족·0벡터면 추가하지 않고 false.
    pub fn push(&mut self, full: &[f32]) -> bool {
        if full.len() < self.dim.max(self.bit_dim) {
            return false;
        }
        let (Some(v), Some(b)) = (truncate_normalize(full, self.dim), truncate_normalize(full, self.bit_dim)) else {
            return false;
        };
        let (c, s) = quantize_i8(&v);
        self.codes.extend_from_slice(&c);
        self.inv_scales.push(s);
        self.bits.extend_from_slice(&sign_bits(&b));
        true
    }

    fn code(&self, i: usize) -> &[i8] {
        &self.codes[i * self.dim..(i + 1) * self.dim]
    }

    fn bit(&self, i: usize) -> &[u64] {
        &self.bits[i * self.words..(i + 1) * self.words]
    }

    fn rescore(&self, q: &[f32], ids: impl Iterator<Item = usize>, k: usize) -> Vec<(u32, f32)> {
        let Some(qn) = truncate_normalize(q, self.dim) else { return vec![] };
        let (qc, qs) = quantize_i8(&qn);
        let mut scored: Vec<(u32, f32)> =
            ids.map(|i| (i as u32, dot_i8(&qc, self.code(i)) as f32 * qs * self.inv_scales[i])).collect();
        top_k(&mut scored, k);
        scored
    }

    pub fn search_exact(&self, q: &[f32], k: usize) -> Vec<(u32, f32)> {
        self.rescore(q, 0..self.len(), k)
    }

    pub fn search_hamming(&self, q: &[f32], candidates: usize, k: usize) -> Vec<(u32, f32)> {
        let Some(qb) = truncate_normalize(q, self.bit_dim) else { return vec![] };
        let qbits = sign_bits(&qb);
        let mut cand: Vec<(u32, f32)> =
            (0..self.len()).map(|i| (i as u32, -(hamming(&qbits, self.bit(i)) as f32))).collect();
        top_k(&mut cand, candidates);
        self.rescore(q, cand.into_iter().map(|c| c.0 as usize), k)
    }

    pub fn search_asym(&self, q: &[f32], candidates: usize, k: usize) -> Vec<(u32, f32)> {
        let Some(qb) = truncate_normalize(q, self.bit_dim) else { return vec![] };
        let mut cand: Vec<(u32, f32)> = (0..self.len()).map(|i| (i as u32, asym_score(&qb, self.bit(i)))).collect();
        top_k(&mut cand, candidates);
        self.rescore(q, cand.into_iter().map(|c| c.0 as usize), k)
    }
}

/// 점수 내림차순 상위 k개만 남긴다(동점은 id 오름차순).
fn top_k(v: &mut Vec<(u32, f32)>, k: usize) {
    let cmp = |a: &(u32, f32), b: &(u32, f32)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
    if k == 0 {
        v.clear();
        return;
    }
    if v.len() > k {
        v.select_nth_unstable_by(k - 1, cmp);
        v.truncate(k);
    }
    v.sort_by(cmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lcg_vecs(n: usize, d: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                (0..d)
                    .map(|_| {
                        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                    })
                    .collect()
            })
            .collect()
    }
    #[test]
    fn exact_finds_itself_first() {
        let vs = lcg_vecs(200, 64, 7);
        let mut ix = FlatIndex::new(32, 64);
        for v in &vs {
            assert!(ix.push(v));
        }
        let top = ix.search_exact(&vs[42], 3);
        assert_eq!(top[0].0, 42);
        assert!(top[0].1 >= top[1].1 && top[1].1 >= top[2].1);
    }
    #[test]
    fn binary_with_all_candidates_equals_exact() {
        let vs = lcg_vecs(100, 64, 9);
        let mut ix = FlatIndex::new(64, 64);
        for v in &vs {
            ix.push(v);
        }
        let q = &vs[5];
        let ids = |r: Vec<(u32, f32)>| r.into_iter().map(|x| x.0).collect::<Vec<_>>();
        assert_eq!(ids(ix.search_hamming(q, 100, 10)), ids(ix.search_exact(q, 10)));
        assert_eq!(ids(ix.search_asym(q, 100, 10)), ids(ix.search_exact(q, 10)));
    }
    #[test]
    fn rejects_short_and_zero_vectors() {
        let mut ix = FlatIndex::new(4, 8);
        assert!(!ix.push(&[1.0; 4]));
        assert!(!ix.push(&[0.0; 8]));
        assert!(ix.is_empty());
    }
    #[test]
    fn k_larger_than_len_is_fine() {
        let mut ix = FlatIndex::new(4, 4);
        ix.push(&[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(ix.search_exact(&[1.0, 0.0, 0.0, 0.0], 10).len(), 1);
        assert_eq!(ix.search_asym(&[1.0, 0.0, 0.0, 0.0], 5, 10).len(), 1);
    }
}
