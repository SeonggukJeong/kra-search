# lindera(형태소 사전)·pdfium 올리기

## lindera
1. `crates/index/Cargo.toml` 의 `lindera = "=X"` 를 바꾼다.
2. `tools/fetch-ko-dic.sh` 의 `LINDERA_VER` 를 새 버전으로. 새 lindera-dictionary 의 `DICTIONARY_FORMAT_VERSION` 이 바뀌었으면 `FMT` 도(캐시 폴더 이름 `<버전>-fmt<형식>`).
3. lindera-ko-dic `build.rs` 의 사전 파일 이름·URL·MD5 가 바뀌었으면 스크립트의 FILE·URL·SHA-256 을 새로 계산해 넣는다.
4. `cargo deny check` — `ureq` 를 끌어오는 크레이트 이름이 바뀌면 `deny.toml` 의 `wrappers` 를 고친다. `bash tools/check-no-net.sh` 통과 확인.
5. `cargo test -p index --release` — 형태소 위치 시험(`morph_drops_particles_and_endings`)이 바뀌면 출력을 보고 의도한 변화인지 확인.
6. `cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --keyword --out docs/m1/eval-keyword.md` 로 점수 변화를 확인.

## pdfium
1. `tools/fetch-pdfium.sh` 의 `TAG` 와 플랫폼별 SHA-256 4개를 새 릴리즈 값으로(`bash tools/fetch-pdfium.sh all` 로 받아 `shasum -a 256`).
2. pdfium-render 의 `pdfium_XXXX` 기능을 같은 번호로(없으면 가장 가까운 아래 번호 — 바인딩 시 모든 심볼을 확인하므로 맞지 않으면 init 이 실패한다).
3. `cargo test -p extract --release --test formats pdf_` 와 릴리즈 스모크(`pdf 3/`).
