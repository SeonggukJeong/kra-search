//! 저장 단위 레코드: 앞 `dim` 차원 int8 코드 + 역스케일 + 앞 `bit_dim` 차원 부호 비트(M0 결정 512/768).
//! 평가(m0eval, f32 캐시)와 제품(디스크)이 같은 레코드에서 점수를 내도록 계산은 여기 한 곳에서만 한다.

use crate::{quantize_i8, sign_bits, truncate_normalize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub dim: usize,
    pub bit_dim: usize,
}

impl Layout {
    /// M0 결정: int8 512차원 + 이진 768비트(스펙 §6.7·§8.2).
    pub const M0: Layout = Layout { dim: 512, bit_dim: 768 };

    pub fn words(&self) -> usize {
        self.bit_dim.div_ceil(64)
    }

    pub fn record_bytes(&self) -> usize {
        self.dim + 4 + self.words() * 8
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub codes: Vec<i8>,
    pub inv_scale: f32,
    pub bits: Vec<u64>,
}

impl Record {
    /// 원본 차원 벡터 → 레코드. 차원이 모자라거나 0벡터·비유한값이면 None.
    pub fn from_full(full: &[f32], l: Layout) -> Option<Record> {
        let v = truncate_normalize(full, l.dim)?;
        let b = truncate_normalize(full, l.bit_dim)?;
        let (codes, inv_scale) = quantize_i8(&v);
        Some(Record { codes, inv_scale, bits: sign_bits(&b) })
    }

    /// 리틀 엔디언: 코드(dim 바이트) · 역스케일(f32) · 비트(u64 × words).
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend(self.codes.iter().map(|c| *c as u8));
        out.extend_from_slice(&self.inv_scale.to_le_bytes());
        for w in &self.bits {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }

    pub fn read_from(b: &[u8], l: Layout) -> Record {
        let codes = b[..l.dim].iter().map(|x| *x as i8).collect();
        let inv_scale = f32::from_le_bytes(b[l.dim..l.dim + 4].try_into().expect("4바이트"));
        let bits = b[l.dim + 4..l.record_bytes()].chunks_exact(8).map(|w| u64::from_le_bytes(w.try_into().expect("8바이트"))).collect();
        Record { codes, inv_scale, bits }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::FlatIndex;

    fn v(seed: u64, n: usize) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect()
    }

    #[test]
    fn bytes_roundtrip() {
        let l = Layout { dim: 32, bit_dim: 64 };
        let r = Record::from_full(&v(1, 64), l).unwrap();
        let mut b = Vec::new();
        r.write_to(&mut b);
        assert_eq!(b.len(), l.record_bytes());
        assert_eq!(Record::read_from(&b, l), r);
    }

    #[test]
    fn m0_layout_size() {
        assert_eq!(Layout::M0.record_bytes(), 512 + 4 + 96);
    }

    /// 평가(f32 캐시 → 레코드)와 제품(디스크에서 읽은 레코드)이 같은 점수를 내야 한다 — 일치 검사의 바탕.
    #[test]
    fn push_and_push_record_search_identically() {
        let l = Layout { dim: 32, bit_dim: 64 };
        let vs: Vec<Vec<f32>> = (0..50).map(|i| v(i, 64)).collect();
        let (mut a, mut b) = (FlatIndex::new(l.dim, l.bit_dim), FlatIndex::new(l.dim, l.bit_dim));
        for x in &vs {
            a.push(x);
            let mut bytes = Vec::new();
            Record::from_full(x, l).unwrap().write_to(&mut bytes);
            b.push_record(&Record::read_from(&bytes, l));
        }
        assert_eq!(a.search_hamming(&vs[3], 20, 10), b.search_hamming(&vs[3], 20, 10));
        assert_eq!(a.search_exact(&vs[7], 10), b.search_exact(&vs[7], 10));
    }

    #[test]
    fn short_or_zero_vectors_have_no_record() {
        let l = Layout { dim: 4, bit_dim: 8 };
        assert!(Record::from_full(&[1.0; 4], l).is_none());
        assert!(Record::from_full(&[0.0; 8], l).is_none());
    }
}
