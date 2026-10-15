# M1 리뷰에서 미룬 작은 항목들 (KI-040)

M1 Task 리뷰(작업 기록부 `.superpowers/sdd/2026-10-14-m1-extract-keyword/progress.md` 의 `minor (deferred)`)에서 미룬 것 가운데
M1 최종 수정에서 고치지 않았고 다른 KI 에도 없는 것만 모았다. 해당 크레이트를 다음에 손볼 때 이 목록을 먼저 본다.
고치면 줄 끝에 `— 해결(커밋)` 을 붙이고 지우지 않는다.

이미 다른 곳에 있어 여기서 뺀 것: 빈 TXT/CSV·`\r` 줄 끝·CSV 구분자(KI-034), PPTX 자리표시자·ZIP 항목 암호·빈 `<w:tc/>`·PPTX 제목 줄바꿈(KI-035),
quick-xml 중복(KI-022), DPAPI 오류 분류(KI-030), 워커가 시작부터 죽는 경우(KI-036).
M1 최종 수정에서 해결: PDF 글자 총량 상한·TXT 디코딩 복사(DEC-036 (5)), 큰 응답 뒤 읽기 버퍼 유지(DEC-033 (2) 재활용),
진단 `FormatCount.timeout` 에 섞인 비정상 종료(`crashed`), decisions.md 덧붙인 문구 구분자·results.md 집계 설명, text.rs 상수 이름 주석,
KI-011 칸 구분자. 이미 해소: `magic::sniff` 호출처 없음(형식 판별에서 씀), norm/table 의 `#[allow(dead_code)]`(없어짐).

## extract
- `walk.rs` 중간의 `use` 문, `push_text` 와 `norm::push` 가 비슷함(계획 지정 구조).
- HWPML·HWP 3.x 파서 오류를 `Unsupported`(미지원)로 분류 — 실제로는 손상인데 진단 통계에서 미지원으로 셈(계획 지정).
- 그룹 안 `ShapeObject::Picture` 캡션 경로 시험 없음.
- 표 캡션은 `push_text`(중첩 컨트롤 빠짐), 새 캡션 경로는 `inner_paragraph` — 둘이 다름.
- `first_element_text` 가 XML 오류를 만나면 잘린 제목을 돌려줌.
- OOXML 시험 빈틈: `slide_paths` 숫자 순서 대체, 중첩 표, styles.xml 없음, document.xml 없음.
- `odf.rs`: 중첩 표 안 반복(repeat) 값 덮어씀(드묾), 중첩 frame 의 End 가 `title_frame` 을 풂, 표 칸 안 각주 순서, `mod` 선언 순서.
- ODS 200만 칸 한도의 최대 메모리 약 200~300MB, 문서 전체 기준이라 보통 크기 시트도 `TooComplex`(DEC-036 (2) 에 기록).
- ODS 어림이 중첩 table 깊이를 따지지 않음, ODS 는 펼친 뒤 5만 행만 가져와 작업량은 줄지 않음, XLSX 셀 하나 오류에 문서 전체 `Corrupt`.
- PDF `page_ok` 의 "10글자 이상" 조건 때문에 짧은 깨진 쪽은 통과(계획 지정).
- `pdf::init` 동시성 시험은 그 시험이 먼저 돌 때만 경합을 만듦, 두 번째 `init` 에 다른 경로를 주면 조용히 무시.
- 워커: 파일당 시간 제한에 워커 시작 시간이 들어감(DEC-033 (1), 재시작 직후 첫 파일 불리), 응답 직후 죽은 워커의 비정상 종료가 다음 파일 탓이 될 수 있음(재시도 1회로 완화).
- 워커: 응답을 세 번 복사(줄 → 문자열 → 역직렬화), 공개 API 문서 주석 없음, 줄 상한·id 불일치·부분 응답 시험 없음.

## platform
- 키 파일: 이름 바꾸기(rename) 뒤 상위 폴더 fsync 없음, 평문 키를 메모리에서 지우지 않음, 임시 파일 확장자 충돌(이론상).

## index
- `finish_flush` 가 조건 없이 Indexed 로 표시 — 지금은 동기 flush + 단일 인스턴스 잠금(DEC-039)이라 안전, 이 전제를 코드 주석으로 남길 것.
- meta 무결성 경계 시험 몇 개 없음, 16진 키 문자열을 메모리에서 지우지 않음.
- 토크나이저 이상 입력 시험 없음(빈 문자열·이모지·결합 문자·41자 이상 토큰).
- `build_query` 가 `tokenizer_for_field` 를 낱말마다 다시 부름, 1,000자 한 낱말이면 1,000항 구절 질의.
- 키워드 버전 파일(`kra-version`) 쓰기가 원자적이지 않음, `upsert` 에서 추가가 실패하면 이전 문서만 지워짐.
- 같은 배치의 이중 upsert, 스키마 불일치 폴더 보존 시험 없음.
- `push_capped` 의 항상 참인 조건(`i + c.len_utf8() > 0`)과 오해할 만한 주석, 상한 시험의 경계 확인이 약해짐.

## extract(워커)
- 입력 64MB마다 워커를 다시 띄우므로(DEC-033) "다시 띄운 직후 첫 파일"이 많아짐 — 시간 제한에 기동 시간이 포함돼 느린 PC 에서 시간 초과가 조금 늘 수 있음(Windows 재측정은 KI-037).

## crawler
- walkdir 오류에 경로가 없으면 `errored` 에 넣지 않음(안전 쪽으로 루트를 넣는 방안).
- 형제 접두어(`/usb` 와 `/usb2`) 시험 없음, `hash_file` 이 `Interrupted` 를 실패로 봄, unix 권한 시험이 root 로 돌면 실패.

## app
- `KeywordIndex::open` 의 일시적 입출력 오류도 전체 재구축(파생 데이터만 잃음).
- `search` 만 실행해도 SQLite 가 `-wal`·`-shm` 을 만듦(정상 동작), 기존 홈 불변·잠금 중 검색 시험 없음.
- 중간 flush 뒤 실패한 실행은 옛 경로가 다음 `index` 까지 검색에 남음, 수정된 파일이 나중 파일이 쓸 내용을 고아로 만들 수 있음(드묾).
- 라이브러리 영문 오류 문구가 그대로 사용자에게 보임(D7).
- pdfium 이 없는 실행에서 PDF 를 옮기면 새 경로는 건너뛰고 옛 경로는 지워져, pdfium 이 돌아온 다음 실행까지 검색에서 빠지고 재사용 대신 다시 추출됨(드묾, 자가 회복).
- pdfium 이 없으면 PDF 가 하나도 없어도 `index` 마다 "PDF 는 이번에 건너뜁니다" 줄이 나옴.
- 요약의 "클라우드 전용이라 건너뜀 N개" 에 파일과 폴더 수가 섞임.
- 잘린 문서(`Stats.truncated`)를 meta 에 남기지 않아 화면에 "앞부분만 색인" 표시를 못 함(스펙 §6.5 `flags.truncated`, M3 화면과 함께).

## diag
- `pdf_pages`·제N조 자동 번호·카드 길이 집계 시험 없음.
- README 의 "최대 3,000개" 고정 문구(`PER_FORMAT_CAP` 을 바꾸면 함께 고칠 것), 거대 문단 시간 시험은 `models/` 없는 CI 에서 건너뜀.
- `diag/bench.rs` import 순서가 rustfmt 와 다름(CI 는 fmt 를 돌리지 않음).

## m0eval
- `experiments.rs` import 순서가 rustfmt 와 다름, `keyword.rs` 의 "문서 id = 목록 순번" 가정에 주석 없음.

## 도구·문서
- `tools/check-no-net.sh` 금지 목록에 hyper-util·h2·native-tls 없음, cargo 영문 오류 문구에 의존.
- `Cargo.lock` 의 itertools 판 변동(Task 14), CLAUDE.md 개발 명령 주석 정렬 한 칸.
