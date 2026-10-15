//! 임베딩 벡터 연산: MRL 축소·정규화, int8 양자화, 이진 부호 비트.

pub mod flat;

/// 앞쪽 `dim`개만 남기고 L2 정규화한다. 차원이 부족하거나 비유한값·0벡터면 `None`.
pub fn truncate_normalize(v: &[f32], dim: usize) -> Option<Vec<f32>> {
    if dim == 0 || dim > v.len() {
        return None;
    }
    let t = &v[..dim];
    if t.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let norm = t.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 1e-12 {
        return None;
    }
    Some(t.iter().map(|x| x / norm).collect())
}

/// 벡터별 스케일 int8 양자화. 반환 = (코드, 역스케일). 값 ≈ code × 역스케일.
pub fn quantize_i8(v: &[f32]) -> (Vec<i8>, f32) {
    let m = v.iter().fold(0f32, |a, x| a.max(x.abs()));
    if m == 0.0 {
        return (vec![0; v.len()], 0.0);
    }
    let s = 127.0 / m;
    (v.iter().map(|x| (x * s).round().clamp(-127.0, 127.0) as i8).collect(), m / 127.0)
}

pub fn dot_i8(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(x, y)| *x as i32 * *y as i32).sum()
}

/// i번째 비트 = v[i] > 0.
pub fn sign_bits(v: &[f32]) -> Vec<u64> {
    let mut out = vec![0u64; v.len().div_ceil(64)];
    for (i, x) in v.iter().enumerate() {
        if *x > 0.0 {
            out[i / 64] |= 1u64 << (i % 64);
        }
    }
    out
}

pub fn hamming(a: &[u64], b: &[u64]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// 실수 검색어 × ±1 문서 비트의 내적(비대칭 점수).
pub fn asym_score(q: &[f32], bits: &[u64]) -> f32 {
    q.iter()
        .enumerate()
        .map(|(i, x)| if (bits[i / 64] >> (i % 64)) & 1 == 1 { *x } else { -*x })
        .sum()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { d / (na * nb) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_truncated_prefix() {
        let v = truncate_normalize(&[3.0, 4.0, 100.0], 2).unwrap();
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
    }
    #[test]
    fn zero_vector_is_rejected() {
        assert!(truncate_normalize(&[0.0, 0.0], 2).is_none());
    }
    #[test]
    fn nan_is_rejected() {
        assert!(truncate_normalize(&[f32::NAN, 1.0], 2).is_none());
    }
    #[test]
    fn dim_larger_than_input_is_rejected() {
        assert!(truncate_normalize(&[1.0], 2).is_none());
    }
    #[test]
    fn quantize_roundtrip_is_close() {
        let v = truncate_normalize(&[0.1, -0.5, 0.3, 0.7], 4).unwrap();
        let (c, s) = quantize_i8(&v);
        for (x, q) in v.iter().zip(&c) {
            assert!((x - *q as f32 * s).abs() < 0.01);
        }
        assert_eq!(c.iter().map(|x| x.unsigned_abs()).max(), Some(127));
    }
    #[test]
    fn quantize_zero_is_zero() {
        assert_eq!(quantize_i8(&[0.0, 0.0]), (vec![0, 0], 0.0));
    }
    #[test]
    fn dot_i8_works() {
        assert_eq!(dot_i8(&[1, -2, 3], &[4, 5, -6]), 4 - 10 - 18);
    }
    #[test]
    fn sign_bits_layout() {
        let mut v = vec![-1.0f32; 70];
        v[0] = 1.0;
        v[65] = 1.0;
        let b = sign_bits(&v);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], 1);
        assert_eq!(b[1], 1 << 1);
    }
    #[test]
    fn hamming_counts_bits() {
        assert_eq!(hamming(&[0b1011], &[0b0001]), 2);
    }
    #[test]
    fn asym_score_signs() {
        let bits = sign_bits(&[1.0, -1.0, 1.0]);
        assert!((asym_score(&[0.5, 0.25, -0.1], &bits) - (0.5 - 0.25 - 0.1)).abs() < 1e-6);
    }
    #[test]
    fn cosine_of_same_is_one() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-6);
    }
}
