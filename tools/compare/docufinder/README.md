# Docufinder 비교 평가 하네스

비슷한 공개 프로그램 Docufinder(Anything, github.com/chrisryugj/Docufinder)를 우리 평가 세트로 재기 위한 것. 결과는 `docs/compare/docufinder.md`.

- Docufinder 는 **BSL 1.1** 이다. 평가·시험(비프로덕션) 사용만 허용되므로, 그쪽 소스는 저장소 밖(임시 폴더)에서 빌드하고 **그쪽 코드를 이 저장소로 가져오지 않는다**. 여기 있는 `kra_eval.rs` 는 우리가 쓴 하네스이고, 그쪽 크레이트 안에 `#[cfg(test)]` 모듈로 넣어 돌린다(앱 바이너리에는 들어가지 않음).
- 하네스는 화면의 '폴더 추가'와 같은 순서로 색인하고, 화면 명령(`commands/search.rs`)과 같은 서비스 함수·기본값(결과 50개, AND, 버전 접기 켬)으로 키워드·하이브리드(의미 검색 켬)·스마트(자연어) 세 모드를 검색한다. 청크 결과는 파일 첫 등장 순서로 한 번씩만 남긴다.

## 재현(macOS Apple Silicon)

```bash
W=<임시 폴더>
# 1. 소스(태그 v3.8.14 = b9f57f7)와 같은 판의 배포본(kordoc·node·onnxruntime 번들)
git clone https://github.com/chrisryugj/Docufinder $W/repo && git -C $W/repo checkout v3.8.14
gh release download v3.8.14 -R chrisryugj/Docufinder -p 'Anything_3.8.14_aarch64.dmg' -D $W
hdiutil attach -readonly -nobrowse -mountpoint $W/mnt $W/Anything_3.8.14_aarch64.dmg
cp -R $W/mnt/Anything.app/Contents/Resources/resources $W/res && hdiutil detach $W/mnt
# 2. 하네스 넣기 + 빌드에 필요한 자리 채우기
cp tools/compare/docufinder/kra_eval.rs $W/repo/src-tauri/src/
#    src-tauri/src/lib.rs 의 `mod perf_bench;` 다음 줄에: #[cfg(test)] mod kra_eval;
mkdir -p $W/repo/dist && echo '<!doctype html>' > $W/repo/dist/index.html
cp -R $W/res/kordoc $W/res/node $W/repo/src-tauri/resources/
cp $W/res/onnxruntime/* $W/repo/src-tauri/resources/onnxruntime/
cp $W/res/pdfium/* $W/repo/src-tauri/resources/pdfium/
cp $W/res/paddleocr/* $W/repo/src-tauri/resources/paddleocr/
(cd $W/repo/src-tauri && cargo test --release --lib kra_eval --no-run)
#    테스트 바이너리 옆에 resources/(node, kordoc/) — 릴리즈 빌드는 실행 파일 옆 번들만 찾는다
ln -s $W/res $W/repo/src-tauri/target/release/deps/resources
# 3. 임베딩 모델(앱이 받는 것과 같은 파일 · SHA-256 은 그쪽 model_downloader.rs 상수와 대조)
#    model_int8.onnx 877e43d3…0f33 · tokenizer.json d607daae…f0a5ab
mkdir -p $W/home/models/kosimcse-roberta-multitask   # huggingface.co/chrisryugj/kosimcse-roberta-multitask-onnx
# 4. 평가 문서 복사본은 우리 제품 평가가 만든 것을 그대로 쓴다(파일 이름·수정일이 같게, DEC-052)
cargo run -p app --release -- --home target/eval-home --now 2026-10-15 eval --set eval/public --set eval/kra --set eval/data --dump target/cmp/ours-hybrid.json
(cd $W/repo/src-tauri && KRA_EVAL_DOCS=$PWD_KRA/target/eval-home/eval-docs KRA_EVAL_HOME=$W/home \
  KRA_EVAL_QUERIES=$PWD_KRA/eval/public/queries.tsv:$PWD_KRA/eval/kra/queries.tsv:$PWD_KRA/eval/data/queries.tsv \
  KRA_EVAL_OUT=$PWD_KRA/target/cmp ORT_DYLIB_PATH=$W/res/onnxruntime/libonnxruntime.dylib \
  cargo test --release --lib kra_eval -- --ignored --nocapture --test-threads 1)
# 5. 같은 지표로 채점
cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --eval eval/data \
  --compare 우리-하이브리드=target/cmp/ours-hybrid.json --compare Docufinder-하이브리드=target/cmp/docufinder-hybrid.json … --out <파일>
```

`$PWD_KRA` 는 이 저장소 절대 경로. 우리 쪽 키워드 단독(모델 없는 제품 경로)은 `--model` 에 없는 파일을 주고 `eval … --keyword-only`.
