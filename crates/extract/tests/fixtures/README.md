# 추출 테스트 샘플

rhwp 저장소(https://github.com/edwardkim/rhwp, MIT, rev 1a76570) `samples/`에서 복사:

| 파일 | 원본 |
|---|---|
| schedule.hwp | 추진일정.hwp |
| schedule.hwpx | 추진일정.hwpx |
| form.hwp | 복학원서.hwp |
| table-text.hwpx | 표-텍스트.hwpx |
| distribution.hwpx | task1768/distribution_doc.hwpx (배포용 플래그, 서울 정보소통광장 공개 결재문서) |
| formatting_table.hml | samples/hml/formatting_table.hml (HWPML 2.91, KI-010 시험) |

직접 만든 파일:
- corrupt.hwp — form.hwp 앞 4KB만 자른 손상 파일
- not_container.hwp — OLE/ZIP이 아닌 임의 헤더
- drm_fasoo.hwp — Fasoo DRM 헤더 시그니처(`\x9b DRMONE`)만 흉내 낸 파일

정책브리핑(korea.kr) 보도자료 첨부 — **공공누리 제1유형(출처표시)**, 평가 세트 `eval/public` 과 같은 파일.
추출 누락 회귀 시험(`tests/coverage.rs`)용으로 고른 것으로, 예전 어댑터(rhwp `build_chunks`)에서 본문이 빠졌던 문서다.

| 파일 | 평가 id | 출처 | 시험하는 것 |
|---|---|---|---|
| notice_clauses.hwpx | P0522 | 보건복지부 "노인을 위한 도시는 있다" 고령친화도시 지정제도 본격 추진 (korea.kr newsId=156783899) | 고시 조문(제N조·항·호)의 본문 없는 제목 줄 |
| press_binary.hwp | P0989 | 산림청 양산산림항공관리소, 통도사 일원서 대대적 합동 캠페인 (korea.kr newsId=156783153) | 바이너리 HWP 본문·표 |
| press_textbox.hwpx | P0169 | 관계부처 합동 10월 일자리전담반(TF) 개최 (korea.kr newsId=156784628) | 글상자 안 글자 |

## 사무 문서·텍스트 (`office/`)
`tools/fixtures/make_office.py` 로 만든 합성 문서(표준 라이브러리 zipfile, 내용은 모두 지어낸 것).
다시 만들 때: `python3 -I tools/fixtures/make_office.py crates/extract/tests/fixtures/office` — ZIP 시각을 고정해 같은 바이트가 나온다.
`encrypted.odt` 는 암호 ODF 의 겉모양(manifest 의 encryption-data + 가짜 암호문)만 흉내 낸 것으로, 실제 암호로는 열리지 않는다.
