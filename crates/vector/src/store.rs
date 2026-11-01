//! 벡터 파일(DEC-048): 머리 32바이트 + 고정 길이 레코드. 슬롯 = 레코드 순번. 추가만 한다.
//! 몇 개가 유효한지는 meta.db 가 기억한다(쓰기 순서: 레코드 추가 → fsync → meta 기록). 열 때 meta 보다 긴 꼬리는 잘라낸다.

use crate::record::{Layout, Record};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: [u8; 8] = *b"KRAVEC01";
pub const VERSION: u32 = 1;
pub const HEADER: u64 = 32;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// 형식 표시·버전·차원이 다름(다른 프로그램의 파일이거나 깨짐)
    BadHeader,
    /// 파일에 레코드가 meta 가 아는 것보다 적다(유실)
    Short { have: u64, want: u64 },
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "벡터 파일 오류: {e}"),
            StoreError::BadHeader => write!(f, "벡터 파일 형식이 다름"),
            StoreError::Short { have, want } => write!(f, "벡터 파일이 짧음({have}/{want})"),
        }
    }
}

fn header(l: Layout) -> [u8; HEADER as usize] {
    let mut h = [0u8; HEADER as usize];
    h[..8].copy_from_slice(&MAGIC);
    h[8..12].copy_from_slice(&VERSION.to_le_bytes());
    h[12..16].copy_from_slice(&(l.dim as u32).to_le_bytes());
    h[16..20].copy_from_slice(&(l.bit_dim as u32).to_le_bytes());
    h
}

/// 머리를 읽어 확인한다. 끝나면 읽기 위치는 첫 레코드.
fn check_header(f: &mut File, l: Layout) -> Result<(), StoreError> {
    let mut h = [0u8; HEADER as usize];
    f.seek(SeekFrom::Start(0))?;
    if f.read_exact(&mut h).is_err() || h != header(l) {
        return Err(StoreError::BadHeader);
    }
    Ok(())
}

pub struct VecFile {
    file: File,
    layout: Layout,
    len: u64,
}

impl VecFile {
    /// 새 파일(있으면 비운다).
    pub fn create(path: &Path, l: Layout) -> io::Result<VecFile> {
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
        file.write_all(&header(l))?;
        file.sync_all()?;
        Ok(VecFile { file, layout: l, len: 0 })
    }

    /// meta 가 아는 길이(`expect`)로 연다. 더 길면 꼬리를 자르고, 짧으면 `Short`.
    pub fn open(path: &Path, l: Layout, expect: u64) -> Result<VecFile, StoreError> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        check_header(&mut file, l)?;
        let size = file.metadata()?.len();
        let rec = l.record_bytes() as u64;
        let have = (size - HEADER) / rec;
        if have < expect {
            return Err(StoreError::Short { have, want: expect });
        }
        let want = HEADER + expect * rec;
        if size != want {
            file.set_len(want)?;
            file.sync_all()?;
        }
        Ok(VecFile { file, layout: l, len: expect })
    }

    /// 레코드를 덧붙이고 디스크에 내린 뒤 첫 슬롯 번호를 돌려준다.
    pub fn append(&mut self, recs: &[Record]) -> io::Result<u64> {
        let first = self.len;
        let mut buf = Vec::with_capacity(recs.len() * self.layout.record_bytes());
        for r in recs {
            r.write_to(&mut buf);
        }
        self.file.seek(SeekFrom::Start(HEADER + self.len * self.layout.record_bytes() as u64))?;
        self.file.write_all(&buf)?;
        self.file.sync_data()?;
        self.len += recs.len() as u64;
        Ok(first)
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// 앞 `len` 개 레코드를 차례로 넘긴다(검색용 읽기·압축). 파일이 그보다 짧으면 `Short`.
pub fn read_each(path: &Path, l: Layout, len: u64, mut f: impl FnMut(u64, Record) -> io::Result<()>) -> Result<(), StoreError> {
    let mut file = File::open(path)?;
    check_header(&mut file, l)?;
    let rec = l.record_bytes() as u64;
    let have = file.metadata()?.len().saturating_sub(HEADER) / rec;
    if have < len {
        return Err(StoreError::Short { have, want: len });
    }
    let mut r = BufReader::with_capacity(1 << 20, file);
    let mut buf = vec![0u8; rec as usize];
    for slot in 0..len {
        r.read_exact(&mut buf)?;
        f(slot, Record::read_from(&buf, l))?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Layout, Record};

    const L: Layout = Layout { dim: 4, bit_dim: 8 };

    fn rec(x: f32) -> Record {
        Record::from_full(&[x, 1.0, -0.5, 0.25, 0.1, -0.2, 0.3, -0.4], L).unwrap()
    }

    fn all(p: &Path, len: u64) -> Result<Vec<(u64, Record)>, StoreError> {
        let mut out = Vec::new();
        read_each(p, L, len, |s, r| {
            out.push((s, r));
            Ok(())
        })?;
        Ok(out)
    }

    #[test]
    fn append_returns_slots_and_reads_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cards.0.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        assert_eq!(f.append(&[rec(1.0), rec(2.0)]).unwrap(), 0);
        assert_eq!(f.append(&[rec(3.0)]).unwrap(), 2);
        assert_eq!(f.len(), 3);
        drop(f);
        let got = all(&p, 3).unwrap();
        assert_eq!(got.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(got[2].1, rec(3.0));
    }

    /// meta 에 기록되기 전에 꺼져 파일에만 남은 꼬리는 열 때 잘라낸다(DEC-048).
    #[test]
    fn open_truncates_unrecorded_tail() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        f.append(&[rec(1.0), rec(2.0), rec(3.0)]).unwrap();
        drop(f);
        let mut f = VecFile::open(&p, L, 2).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(std::fs::metadata(&p).unwrap().len(), HEADER + 2 * L.record_bytes() as u64);
        assert_eq!(f.append(&[rec(9.0)]).unwrap(), 2, "잘라낸 자리부터 다시 쓴다");
    }

    #[test]
    fn short_file_and_bad_header_are_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        f.append(&[rec(1.0)]).unwrap();
        drop(f);
        assert!(matches!(VecFile::open(&p, L, 2), Err(StoreError::Short { have: 1, want: 2 })));
        assert!(matches!(all(&p, 2), Err(StoreError::Short { have: 1, want: 2 })));
        assert!(matches!(VecFile::open(&p, Layout { dim: 8, bit_dim: 8 }, 0), Err(StoreError::BadHeader)), "다른 차원의 파일");
        std::fs::write(&p, b"garbage").unwrap();
        assert!(matches!(VecFile::open(&p, L, 0), Err(StoreError::BadHeader)));
    }

    /// Review Focus 5 바탕: 쓰는 쪽이 더 덧붙여도 읽는 쪽은 주어진 길이만 읽는다.
    #[test]
    fn reader_sees_only_given_len() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        f.append(&[rec(1.0), rec(2.0), rec(3.0)]).unwrap();
        assert_eq!(all(&p, 2).unwrap().len(), 2);
    }
}
