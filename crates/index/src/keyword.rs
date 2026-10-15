//! 키워드 색인(tantivy BM25). 문서 = 내용(content) 하나, doc_id = meta.db 의 content id.
//! 필드 가중치(스펙 §8.2): 제목 > 파일명 > 소제목 > 본문. 본문은 형태소 필드 + 한글 bigram 필드(다글자 낱말은 bigram 구절 일치).

use crate::tokenize::{register, Analyzers, BIGRAM, MORPH};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, INDEXED, STORED};
use tantivy::tokenizer::TokenStream;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, TantivyError, Term};

pub const BODY_MAX_CHARS: usize = 5_000_000;
/// 스키마(schema())·토큰 규칙(tokenize.rs)·lindera·사전 버전 중 하나라도 바꾸면 1 올린다.
/// M4 개인정보 치환이 들어오면 원문 식별번호가 든 색인을 버리기 위해서도 올린다.
pub const KEYWORD_VERSION: u32 = 1;
pub const VERSION_FILE: &str = "kra-version";
const MAX_QUERY_CHARS: usize = 1_000;
const MAX_QUERY_WORDS: usize = 32;
/// 색인 쓰기 메모리 — 저사양 PC 를 생각해 작게(tantivy 최소 15MB/스레드).
const WRITER_BYTES: usize = 50_000_000;

/// 키워드 필드 가중치(스펙 §8.2, DEC-038 초기값). M2 실험(m0eval)이 조정한다 — 검색할 때만 쓰므로 바꿔도 다시 색인할 필요 없다.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeywordWeights {
    pub title: f32,
    pub filename: f32,
    pub heading: f32,
    pub body: f32,
    /// bigram 구절 일치에 곱하는 배수
    pub bigram: f32,
}

impl Default for KeywordWeights {
    fn default() -> Self {
        KeywordWeights { title: 3.0, filename: 2.5, heading: 2.0, body: 1.0, bigram: 0.5 }
    }
}

/// 소제목·본문은 블록마다 값 하나. 한 값에 본문 전체(최대 500만 자)를 넣으면 토큰을 한꺼번에 만들어
/// 1.2GB 까지 올라간다(리뷰 실측: 100만 자 295MB, 500만 자 1.21GB). 값으로 나누면 가장 큰 블록만큼만 쓴다.
pub struct KeywordDoc {
    pub id: u64,
    pub title: String,
    pub filenames: String,
    pub headings: Vec<String>,
    pub body: Vec<String>,
}

fn stem(name: &str) -> &str {
    Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name)
}

/// 값 하나의 최대 글자 수. 토크나이저는 값 전체의 토큰을 한꺼번에 만들므로(100만 자 295MB), 블록 하나가 아무리 커도
/// (공백 줄 없는 TXT·CSV 덤프 = 한 블록 500만 자) 이 크기로 쪼개 메모리를 이 값에 비례하게 묶는다(DEC-041).
pub const MAX_VALUE_CHARS: usize = 20_000;

/// 블록을 넣되 합계 `left` 글자를 넘지 않게(넘는 블록은 잘라 넣고, 그 뒤로는 넣지 않는다).
/// 한 블록이 MAX_VALUE_CHARS 보다 길면 공백·줄바꿈 근처에서 여러 값으로 쪼갠다(없으면 글자 경계에서 자른다).
fn push_capped(dst: &mut Vec<String>, left: &mut usize, text: &str) {
    if *left == 0 || text.trim().is_empty() {
        return;
    }
    let mut rest = match text.char_indices().nth(*left) {
        Some((end, _)) => &text[..end],
        None => text,
    };
    *left -= rest.chars().count();
    while !rest.is_empty() {
        let cut = match rest.char_indices().nth(MAX_VALUE_CHARS) {
            None => rest.len(),
            Some((hard, _)) => {
                // 한도 안의 마지막 공백 뒤에서 자른다(앞부분이 비지 않을 때만).
                match rest[..hard].char_indices().rev().find(|(_, c)| c.is_whitespace()) {
                    Some((i, c)) if i + c.len_utf8() > 0 => i + c.len_utf8(),
                    _ => hard,
                }
            }
        };
        let (piece, tail) = rest.split_at(cut);
        if !piece.trim().is_empty() {
            dst.push(piece.to_string());
        }
        rest = tail;
    }
}

impl KeywordDoc {
    pub fn new(id: u64, doc: Option<&doc::Document>, file_names: &[String]) -> KeywordDoc {
        let filenames = file_names.iter().map(|n| stem(n)).collect::<Vec<_>>().join("\n");
        let title = doc
            .and_then(|d| d.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| file_names.first().map(|n| stem(n).to_string()).unwrap_or_default());
        let (mut headings, mut body) = (Vec::new(), Vec::new());
        let (mut head_left, mut body_left) = (BODY_MAX_CHARS, BODY_MAX_CHARS);
        if let Some(d) = doc {
            for b in &d.blocks {
                if b.kind == doc::BlockKind::Heading {
                    push_capped(&mut headings, &mut head_left, &b.text);
                } else {
                    push_capped(&mut body, &mut body_left, &b.text);
                }
            }
        }
        KeywordDoc { id, title, filenames, headings, body }
    }
}

struct Fields {
    doc_id: Field,
    /// (형태소 필드, bigram 필드) — 제목, 파일명, 소제목, 본문 순. 가중치는 검색할 때 KeywordWeights 로 준다.
    pairs: [(Field, Field); 4],
}

fn schema() -> (Schema, Fields) {
    let mut sb = Schema::builder();
    let doc_id = sb.add_u64_field("doc_id", FAST | INDEXED | STORED);
    let text = |tok: &str| {
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default().set_tokenizer(tok).set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
    };
    let mut pair = |name: &str| (sb.add_text_field(&format!("{name}_m"), text(MORPH)), sb.add_text_field(&format!("{name}_b"), text(BIGRAM)));
    let pairs = [pair("title"), pair("filename"), pair("heading"), pair("body")];
    (sb.build(), Fields { doc_id, pairs })
}

pub struct KeywordIndex {
    index: Index,
    reader: IndexReader,
    writer: Option<IndexWriter>,
    f: Fields,
}

impl KeywordIndex {
    pub fn open(dir: &Path, a: &Analyzers) -> tantivy::Result<KeywordIndex> {
        let (schema, f) = schema();
        std::fs::create_dir_all(dir)?;
        let version_path = dir.join(VERSION_FILE);
        let index = if dir.join("meta.json").exists() {
            let index = Index::open_in_dir(dir)?;
            // 저장된 스키마를 그대로 열고 지금 schema() 의 Field 번호를 쓰므로, 필드가 바뀌었으면 엉뚱한 필드를 찾는다 → 거부.
            if index.schema() != schema {
                return Err(TantivyError::SchemaError("키워드 색인 스키마가 다름 — 다시 색인 필요".into()));
            }
            // 스키마가 같아도 토큰 규칙·사전이 바뀌면 색인 토큰과 검색어 토큰이 어긋난다 → 버전 파일로 확인.
            let stored = std::fs::read_to_string(&version_path).ok().and_then(|s| s.trim().parse::<u32>().ok());
            if stored != Some(KEYWORD_VERSION) {
                return Err(TantivyError::SchemaError(format!("키워드 색인 버전 {stored:?} ≠ {KEYWORD_VERSION} — 다시 색인 필요")));
            }
            index
        } else {
            let index = Index::create_in_dir(dir, schema)?;
            // 만든 직후에 쓴다. 그 사이에 죽으면 다음 open 이 "버전 없음"으로 Err → 다시 만든다(손실 없음, 빈 색인이었음).
            std::fs::write(&version_path, KEYWORD_VERSION.to_string())?;
            index
        };
        Self::wrap(index, f, a)
    }

    pub fn in_ram(a: &Analyzers) -> tantivy::Result<KeywordIndex> {
        let (schema, f) = schema();
        Self::wrap(Index::create_in_ram(schema), f, a)
    }

    fn wrap(index: Index, f: Fields, a: &Analyzers) -> tantivy::Result<KeywordIndex> {
        register(&index, a);
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        Ok(KeywordIndex { index, reader, writer: None, f })
    }

    /// 쓰기 잠금은 실제로 쓸 때만 잡는다(검색만 하는 프로세스가 잠그지 않도록).
    fn writer(&mut self) -> tantivy::Result<&mut IndexWriter> {
        if self.writer.is_none() {
            self.writer = Some(self.index.writer_with_num_threads(1, WRITER_BYTES)?);
        }
        Ok(self.writer.as_mut().expect("writer"))
    }

    pub fn upsert(&mut self, d: &KeywordDoc) -> tantivy::Result<()> {
        let [(tm, tb), (fm, fb), (hm, hb), (bm, bb)] = self.f.pairs;
        let doc_id = self.f.doc_id;
        let mut doc = TantivyDocument::default();
        doc.add_u64(doc_id, d.id);
        for (m, b, text) in [(tm, tb, &d.title), (fm, fb, &d.filenames)] {
            if !text.is_empty() {
                doc.add_text(m, text);
                doc.add_text(b, text);
            }
        }
        // 블록마다 값 하나: tantivy 는 값마다 따로 토큰화하고 값 사이에 위치 간격(POSITION_GAP)을 둔다.
        for (m, b, values) in [(hm, hb, &d.headings), (bm, bb, &d.body)] {
            for text in values {
                doc.add_text(m, text);
                doc.add_text(b, text);
            }
        }
        let w = self.writer()?;
        w.delete_term(Term::from_field_u64(doc_id, d.id));
        w.add_document(doc)?;
        Ok(())
    }

    pub fn delete(&mut self, id: u64) -> tantivy::Result<()> {
        let doc_id = self.f.doc_id;
        self.writer()?.delete_term(Term::from_field_u64(doc_id, id));
        Ok(())
    }

    pub fn commit(&mut self) -> tantivy::Result<u64> {
        let op = self.writer()?.commit()?;
        self.reader.reload()?;
        Ok(op)
    }

    pub fn opstamp(&self) -> u64 {
        self.index.load_metas().map(|m| m.opstamp).unwrap_or(0)
    }

    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    fn build_query(&self, query: &str, w: &KeywordWeights) -> tantivy::Result<Option<Box<dyn Query>>> {
        let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
        let mut words: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for word in query.split_whitespace().take(MAX_QUERY_WORDS) {
            let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            let ws = [w.title, w.filename, w.heading, w.body];
            for (&(mf, bf), &fw) in self.f.pairs.iter().zip(&ws) {
                let mut morph = self.index.tokenizer_for_field(mf)?;
                let mut ts = morph.token_stream(word);
                while let Some(tok) = ts.next() {
                    let tq = TermQuery::new(Term::from_field_text(mf, &tok.text), IndexRecordOption::WithFreqs);
                    clauses.push((Occur::Should, Box::new(BoostQuery::new(Box::new(tq), fw))));
                }
                let mut bigram = self.index.tokenizer_for_field(bf)?;
                let mut ts = bigram.token_stream(word);
                let mut terms: Vec<(usize, Term)> = Vec::new();
                while let Some(tok) = ts.next() {
                    terms.push((tok.position, Term::from_field_text(bf, &tok.text)));
                }
                let bq: Box<dyn Query> = match terms.len() {
                    0 => continue,
                    1 => Box::new(TermQuery::new(terms.pop().expect("1개").1, IndexRecordOption::WithFreqs)),
                    _ => Box::new(PhraseQuery::new_with_offset(terms)), // 2개 미만이면 패닉하므로 위에서 나눔
                };
                clauses.push((Occur::Should, Box::new(BoostQuery::new(bq, fw * w.bigram))));
            }
            if !clauses.is_empty() {
                words.push((Occur::Should, Box::new(BooleanQuery::new(clauses))));
            }
        }
        Ok((!words.is_empty()).then(|| Box::new(BooleanQuery::new(words)) as Box<dyn Query>))
    }

    pub fn search(&self, query: &str, limit: usize) -> tantivy::Result<Vec<(u64, f32)>> {
        self.search_with(query, limit, &KeywordWeights::default())
    }

    pub fn search_with(&self, query: &str, limit: usize, w: &KeywordWeights) -> tantivy::Result<Vec<(u64, f32)>> {
        let Some(q) = self.build_query(query, w)? else { return Ok(Vec::new()) };
        self.top_ids(&*q, limit)
    }

    /// 검색어 전체의 형태소가 순서대로 나오는 문서(제목·소제목·본문). 정확 구절 일치 가산용(스펙 §8.3). 형태소가 2개 미만이면 빈 목록.
    pub fn phrase_ids(&self, query: &str, limit: usize) -> tantivy::Result<Vec<u64>> {
        let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for &(mf, _) in [self.f.pairs[0], self.f.pairs[2], self.f.pairs[3]].iter() {
            let mut tok = self.index.tokenizer_for_field(mf)?;
            let mut ts = tok.token_stream(&query);
            let mut terms: Vec<(usize, Term)> = Vec::new();
            while let Some(t) = ts.next() {
                terms.push((t.position, Term::from_field_text(mf, &t.text)));
            }
            if terms.len() < 2 {
                return Ok(Vec::new());
            }
            clauses.push((Occur::Should, Box::new(PhraseQuery::new_with_offset(terms))));
        }
        Ok(self.top_ids(&BooleanQuery::new(clauses), limit)?.into_iter().map(|x| x.0).collect())
    }

    fn top_ids(&self, q: &dyn Query, limit: usize) -> tantivy::Result<Vec<(u64, f32)>> {
        let searcher = self.reader.searcher();
        let hits = searcher.search(q, &TopDocs::with_limit(limit.max(1)).order_by_score())?;
        let mut out = Vec::with_capacity(hits.len());
        for (score, addr) in hits {
            let d: TantivyDocument = searcher.doc(addr)?;
            if let Some(id) = d.get_first(self.f.doc_id).and_then(|v| v.as_u64()) {
                out.push((id, score));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Block, BlockKind, Document};

    fn analyzers() -> Analyzers {
        Analyzers::load().unwrap()
    }

    fn d(title: Option<&str>, heads: &[&str], body: &[&str]) -> Document {
        let mut blocks: Vec<Block> = heads.iter().map(|h| Block { kind: BlockKind::Heading, text: h.to_string() }).collect();
        blocks.extend(body.iter().map(|b| Block { kind: BlockKind::Paragraph, text: b.to_string() }));
        Document { title: title.map(String::from), blocks }
    }

    fn ids(kw: &KeywordIndex, q: &str) -> Vec<u64> {
        kw.search(q, 10).unwrap().into_iter().map(|x| x.0).collect()
    }

    fn index_of(docs: &[(u64, Document, &str)]) -> KeywordIndex {
        let mut kw = KeywordIndex::in_ram(&analyzers()).unwrap();
        for (id, doc, name) in docs {
            kw.upsert(&KeywordDoc::new(*id, Some(doc), &[name.to_string()])).unwrap();
        }
        kw.commit().unwrap();
        kw
    }

    fn ids_w(kw: &KeywordIndex, q: &str, w: &KeywordWeights) -> Vec<u64> {
        kw.search_with(q, 10, w).unwrap().into_iter().map(|x| x.0).collect()
    }

    #[test]
    fn weights_change_field_priority() {
        let kw = index_of(&[
            (1, d(None, &[], &["이 문서는 내부통제 관련 회의 결과를 정리했다."]), "회의록.hwp"),
            (2, d(Some("내부통제규정 개정안"), &[], &["개정 이유"]), "개정안.hwp"),
        ]);
        assert_eq!(ids_w(&kw, "내부통제", &KeywordWeights::default()), ids(&kw, "내부통제"), "기본값 = 기존 search");
        assert_eq!(ids(&kw, "내부통제"), vec![2, 1]);
        let body_first = KeywordWeights { title: 0.1, filename: 0.1, heading: 0.1, body: 5.0, bigram: 0.5 };
        assert_eq!(ids_w(&kw, "내부통제", &body_first), vec![1, 2]);
    }

    #[test]
    fn phrase_ids_need_morphemes_in_order() {
        let kw = index_of(&[
            (1, d(None, &[], &["내부통제 규정을 개정했다"]), "a.hwp"),
            (2, d(None, &[], &["규정과 내부통제를 따로 본다"]), "b.hwp"),
            (3, d(None, &["내부통제 규정"], &["본문"]), "c.hwp"),
        ]);
        let mut got = kw.phrase_ids("내부통제 규정", 10).unwrap();
        got.sort();
        assert_eq!(got, vec![1, 3]);
        assert!(kw.phrase_ids("규정", 10).unwrap().is_empty(), "형태소 하나는 구절이 아니다");
    }

    #[test]
    fn title_beats_body() {
        let kw = index_of(&[
            (1, d(None, &[], &["이 문서는 내부통제 관련 회의 결과를 정리했다."]), "회의록.hwp"),
            (2, d(Some("내부통제규정 개정안"), &[], &["개정 이유"]), "개정안.hwp"),
        ]);
        assert_eq!(ids(&kw, "내부통제"), vec![2, 1]);
    }

    #[test]
    fn compound_and_spaced_queries_match() {
        let kw = index_of(&[
            (1, d(None, &[], &["더비온 행사를 운영한다.", "내부통제규정을 개정했다."]), "a.hwp"),
            (2, d(None, &[], &["경마 실적 보고"]), "b.hwp"),
        ]);
        assert_eq!(ids(&kw, "더비온"), vec![1], "사전에 없는 합성어는 bigram 이 찾는다");
        assert_eq!(ids(&kw, "통제 규정"), vec![1]);
        assert_eq!(ids(&kw, "달렸다"), Vec::<u64>::new());
    }

    #[test]
    fn filename_without_extension_is_searchable() {
        let kw = index_of(&[(7, d(None, &[], &[]), "2026 렛츠런파크 안전점검 결과.hwp")]);
        assert_eq!(ids(&kw, "안전점검"), vec![7]);
        assert_eq!(ids(&kw, "hwp"), Vec::<u64>::new(), "확장자는 색인하지 않는다");
    }

    #[test]
    fn upsert_replaces_and_delete_removes() {
        let mut kw = index_of(&[(1, d(None, &[], &["사과 배"]), "a.txt")]);
        kw.upsert(&KeywordDoc::new(1, Some(&d(None, &[], &["포도"])), &["a.txt".into()])).unwrap();
        kw.commit().unwrap();
        assert_eq!(ids(&kw, "사과"), Vec::<u64>::new());
        assert_eq!(ids(&kw, "포도"), vec![1]);
        assert_eq!(kw.num_docs(), 1);
        kw.delete(1).unwrap();
        kw.commit().unwrap();
        assert_eq!(kw.num_docs(), 0);
    }

    #[test]
    fn reopen_keeps_documents_and_opstamp() {
        let dir = tempfile::tempdir().unwrap();
        let a = analyzers();
        let op = {
            let mut kw = KeywordIndex::open(dir.path(), &a).unwrap();
            kw.upsert(&KeywordDoc::new(3, Some(&d(None, &[], &["경마 시행 계획"])), &["x.hwp".into()])).unwrap();
            kw.commit().unwrap()
        };
        let kw = KeywordIndex::open(dir.path(), &a).unwrap();
        assert_eq!(ids(&kw, "경마"), vec![3]);
        assert_eq!(kw.opstamp(), op);
    }

    #[test]
    fn version_or_schema_mismatch_is_rejected() {
        let a = analyzers();
        // 버전 파일이 다르면 거부(토큰 규칙이 바뀐 옛 색인)
        let dir = tempfile::tempdir().unwrap();
        KeywordIndex::open(dir.path(), &a).unwrap().commit().unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(VERSION_FILE)).unwrap(), KEYWORD_VERSION.to_string());
        std::fs::write(dir.path().join(VERSION_FILE), "999").unwrap();
        assert!(KeywordIndex::open(dir.path(), &a).is_err());
        // 버전 파일이 없어도 거부(만들다 죽은 색인)
        std::fs::remove_file(dir.path().join(VERSION_FILE)).unwrap();
        assert!(KeywordIndex::open(dir.path(), &a).is_err());
        // 스키마가 다른 색인(필드가 바뀐 옛 버전)은 버전 파일이 맞아도 거부
        let other = tempfile::tempdir().unwrap();
        let mut sb = Schema::builder();
        sb.add_u64_field("doc_id", FAST | INDEXED | STORED);
        Index::create_in_dir(other.path(), sb.build()).unwrap();
        std::fs::write(other.path().join(VERSION_FILE), KEYWORD_VERSION.to_string()).unwrap();
        assert!(KeywordIndex::open(other.path(), &a).is_err());
    }

    #[test]
    fn keyword_doc_keeps_blocks_as_values_and_caps_total() {
        let block = "가".repeat(1_000_000);
        let body: Vec<&str> = std::iter::repeat_n(block.as_str(), 6).chain(["", "  "]).collect();
        let doc = d(None, &["머리", ""], &body);
        let k = KeywordDoc::new(1, Some(&doc), &["a.hwp".into()]);
        assert_eq!(k.headings, vec!["머리".to_string()], "빈 블록은 뺀다");
        assert_eq!(k.body.len(), BODY_MAX_CHARS / MAX_VALUE_CHARS, "500만 자에서 멈춘다");
        assert_eq!(k.body.iter().map(|b| b.chars().count()).sum::<usize>(), BODY_MAX_CHARS);
        assert!(k.body.iter().all(|b| b.chars().count() <= MAX_VALUE_CHARS));
        // 상한 경계에 걸친 블록은 잘라 넣는다(블록은 서로 다른 값)
        let doc = d(None, &[], &[&"나".repeat(BODY_MAX_CHARS - 3), "다라마바사"]);
        let k = KeywordDoc::new(2, Some(&doc), &[]);
        assert_eq!(k.body.last().unwrap(), "다라마");
        assert_eq!(k.body.iter().map(|b| b.chars().count()).sum::<usize>(), BODY_MAX_CHARS);
    }

    #[test]
    fn huge_single_block_is_split_into_bounded_values() {
        // 공백 줄 없는 덤프: 100만 자 한 블록 → 값 여러 개, 모두 한도 이하, 글자는 그대로(공백 경계에서 자름)
        let text = "한국마사회 경마 시행 계획 ".repeat(1_000_000 / 13 + 1);
        let doc = d(None, &[], &[text.as_str()]);
        let k = KeywordDoc::new(1, Some(&doc), &[]);
        assert!(k.body.len() > 1);
        assert!(k.body.iter().all(|b| b.chars().count() <= MAX_VALUE_CHARS));
        assert_eq!(k.body.concat(), text);
        // 공백 경계에서 잘렸으니 낱말이 반으로 갈라지지 않는다
        assert!(k.body.iter().all(|b| b.ends_with(' ')));
        // 공백 없는 덤프는 글자 경계에서 자르되 합계는 보존
        let solid = "가".repeat(1_000_000);
        let k = KeywordDoc::new(2, Some(&d(None, &[], &[solid.as_str()])), &[]);
        assert!(k.body.len() > 1 && k.body.iter().all(|b| b.chars().count() <= MAX_VALUE_CHARS));
        assert_eq!(k.body.concat(), solid);
    }

    #[test]
    fn blocks_are_searchable_and_do_not_join_into_phrases() {
        // 블록 경계를 넘는 bigram 은 만들어지지 않는다: "통제" | "규정" 두 블록에 "제규" bigram 이 없다.
        let kw = index_of(&[
            (1, d(None, &["안전 점검"], &["경마 시행 계획", "내부통제", "규정 개정"]), "a.hwp"),
            (2, d(None, &[], &["내부통제규정 개정"]), "b.hwp"),
        ]);
        assert_eq!(ids(&kw, "시행"), vec![1]);
        assert_eq!(ids(&kw, "점검"), vec![1]);
        let bigram_only = kw.search("제규", 10).unwrap();
        assert_eq!(bigram_only.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2], "블록을 넘는 bigram 은 없다");
    }

    /// 큰 문서 하나(500만 자, 1,000 블록) 색인 — 메모리 실측용(Task 18 Step 5):
    /// `cargo test -p index --release big_document_indexes_by_block -- --ignored` 를 `/usr/bin/time -l`(macOS) 로 감싸 최대 RSS 를 본다.
    #[test]
    #[ignore]
    fn big_document_indexes_by_block() {
        let para = "한국마사회 경마 시행 계획 보고 ".repeat(5_000_000 / 1_000 / 18 + 1);
        let blocks: Vec<&str> = std::iter::repeat_n(para.as_str(), 1_000).collect();
        let kw = index_of(&[(1, d(None, &[], &blocks), "큰문서.txt")]);
        assert_eq!(ids(&kw, "시행"), vec![1]);
    }

    /// 공백 줄 없는 500만 자 한 블록(TXT·CSV 덤프) — 값으로 쪼개져 메모리가 묶이는지 실측용.
    #[test]
    #[ignore]
    fn big_single_block_indexes_bounded() {
        let text = "한국마사회 경마 시행 계획 보고 ".repeat(BODY_MAX_CHARS / 17 + 1);
        let kw = index_of(&[(1, d(None, &[], &[text.as_str()]), "덤프.txt")]);
        assert_eq!(ids(&kw, "시행"), vec![1]);
    }

    #[test]
    fn fullwidth_query_matches_halfwidth_text() {
        let kw = index_of(&[(1, d(None, &[], &["KRA-2026 문서번호"]), "a.hwp")]);
        assert_eq!(ids(&kw, "ＫＲＡ"), vec![1]);
        assert_eq!(ids(&kw, "kra"), vec![1]);
    }

    #[test]
    fn odd_queries_do_not_panic() {
        let kw = index_of(&[(1, d(Some("가"), &[], &["가나다 라마바"]), "a.hwp")]);
        let long = "가".repeat(5_000);
        for q in ["", "   ", "!!!", "\"", "'\"(){}[]", "가", long.as_str(), "가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나 다 라 마"] {
            let _ = kw.search(q, 10).unwrap();
        }
        assert!(ids(&kw, "").is_empty());
    }
}
