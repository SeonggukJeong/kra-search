# 추출 테스트 샘플

rhwp 저장소(https://github.com/edwardkim/rhwp, MIT, rev 1a76570) `samples/`에서 복사:

| 파일 | 원본 |
|---|---|
| schedule.hwp | 추진일정.hwp |
| schedule.hwpx | 추진일정.hwpx |
| form.hwp | 복학원서.hwp |
| table-text.hwpx | 표-텍스트.hwpx |
| distribution.hwpx | task1768/distribution_doc.hwpx (배포용 플래그, 서울 정보소통광장 공개 결재문서) |

직접 만든 파일:
- corrupt.hwp — form.hwp 앞 4KB만 자른 손상 파일
- not_container.hwp — OLE/ZIP이 아닌 임의 헤더
- drm_fasoo.hwp — Fasoo DRM 헤더 시그니처(`\x9b DRMONE`)만 흉내 낸 파일
