//! llama.cpp 위의 안전한 임베딩 엔진. 평균 풀링, 입력 잘라내기, 토큰 합 기준 묶음 처리.

use llama_sys::*;
use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct EngineOptions {
    pub threads: usize,
    /// 입력 하나의 최대 토큰이자 한 번에 처리하는 토큰 합의 상한(n_ubatch).
    pub max_tokens: usize,
    /// Windows: CPU 변형 DLL(ggml-cpu-*.dll)이 있는 폴더. 정적 빌드에서는 무시된다.
    pub backend_dir: Option<PathBuf>,
}

/// 원본 차원 임베딩(정규화 전).
pub struct Embedding {
    pub vector: Vec<f32>,
    pub n_tokens: usize,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("모델 파일이 없습니다: {0}")]
    ModelNotFound(String),
    #[error("모델을 불러오지 못했습니다: {0}")]
    ModelLoad(String),
    #[error("추론 컨텍스트를 만들지 못했습니다")]
    Context,
    #[error("CPU 백엔드를 불러오지 못했습니다: {0}")]
    Backend(String),
    #[error("빈 입력은 임베딩할 수 없습니다")]
    EmptyInput,
    #[error("토큰화 실패")]
    Tokenize,
    #[error("추론 실패(code {0})")]
    Decode(i32),
}

static BACKEND: OnceLock<Result<(), String>> = OnceLock::new();

fn init_backend(dir: Option<&Path>) -> Result<(), EmbedError> {
    BACKEND
        .get_or_init(|| unsafe {
            if let Some(d) = dir {
                let c = CString::new(d.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
                ggml_backend_load_all_from_path(c.as_ptr());
            }
            llama_backend_init();
            Ok(())
        })
        .clone()
        .map_err(EmbedError::Backend)
}

/// 백엔드만 초기화한다(모델 없이 `system_info`를 보려 할 때).
pub fn init(backend_dir: Option<&Path>) -> Result<(), EmbedError> {
    init_backend(backend_dir)
}

/// llama.cpp가 선택한 CPU 기능(진단용).
pub fn system_info() -> String {
    unsafe { CStr::from_ptr(llama_print_system_info()).to_string_lossy().into_owned() }
}

pub struct Embedder {
    model: *mut llama_model,
    ctx: *mut llama_context,
    vocab: *const llama_vocab,
    n_embd: usize,
    max_tokens: usize,
    encoder_only: bool,
}

// llama 컨텍스트는 한 번에 한 스레드에서만 쓰며(&mut self), 스레드 간 이동은 안전하다.
unsafe impl Send for Embedder {}

const MAX_SEQS: usize = 8;

impl Embedder {
    pub fn load(model_path: &Path, opts: &EngineOptions) -> Result<Self, EmbedError> {
        if !model_path.is_file() {
            return Err(EmbedError::ModelNotFound(model_path.display().to_string()));
        }
        init_backend(opts.backend_dir.as_deref())?;
        let cpath = CString::new(model_path.to_string_lossy().as_bytes())
            .map_err(|_| EmbedError::ModelNotFound(model_path.display().to_string()))?;
        unsafe {
            let mut mp = llama_model_default_params();
            mp.load_mode = llama_load_mode_LLAMA_LOAD_MODE_MMAP;
            let model = llama_model_load_from_file(cpath.as_ptr(), mp);
            if model.is_null() {
                return Err(EmbedError::ModelLoad(model_path.display().to_string()));
            }
            let mut cp = llama_context_default_params();
            cp.embeddings = true;
            cp.pooling_type = llama_pooling_type_LLAMA_POOLING_TYPE_MEAN;
            cp.n_ctx = opts.max_tokens as u32;
            cp.n_batch = opts.max_tokens as u32;
            cp.n_ubatch = opts.max_tokens as u32;
            cp.n_seq_max = MAX_SEQS as u32;
            cp.n_threads = opts.threads as i32;
            cp.n_threads_batch = opts.threads as i32;
            let ctx = llama_init_from_model(model, cp);
            if ctx.is_null() {
                llama_model_free(model);
                return Err(EmbedError::Context);
            }
            Ok(Self {
                model,
                ctx,
                vocab: llama_model_get_vocab(model),
                n_embd: llama_model_n_embd_out(model) as usize,
                max_tokens: opts.max_tokens,
                encoder_only: llama_model_has_encoder(model) && !llama_model_has_decoder(model),
            })
        }
    }

    pub fn n_embd(&self) -> usize {
        self.n_embd
    }

    fn tokenize(&self, text: &str) -> Result<Vec<llama_token>, EmbedError> {
        let call = |buf: &mut Vec<llama_token>| unsafe {
            llama_tokenize(
                self.vocab,
                text.as_ptr() as *const c_char,
                text.len() as i32,
                buf.as_mut_ptr(),
                buf.len() as i32,
                true,
                false,
            )
        };
        let mut buf = vec![0 as llama_token; text.len() + 8];
        let mut n = call(&mut buf);
        if n < 0 {
            buf.resize((-n) as usize, 0);
            n = call(&mut buf);
            if n < 0 {
                return Err(EmbedError::Tokenize);
            }
        }
        buf.truncate(n as usize);
        Ok(buf)
    }

    pub fn token_count(&self, text: &str) -> Result<usize, EmbedError> {
        Ok(self.tokenize(text)?.len())
    }

    /// 입력마다 원본 차원 임베딩을 돌려준다. 공백뿐인 입력이 있으면 `EmptyInput`.
    pub fn embed(&mut self, inputs: &[String]) -> Result<Vec<Embedding>, EmbedError> {
        let mut toks = Vec::with_capacity(inputs.len());
        for s in inputs {
            if s.trim().is_empty() {
                return Err(EmbedError::EmptyInput);
            }
            let mut t = self.tokenize(s)?;
            let truncated = t.len() > self.max_tokens;
            t.truncate(self.max_tokens);
            toks.push((t, truncated));
        }
        // 양방향 어텐션이므로 한 묶음의 토큰 합이 n_ubatch(max_tokens)를 넘지 않게 묶는다.
        let mut out = Vec::with_capacity(inputs.len());
        let mut i = 0;
        while i < toks.len() {
            let mut j = i;
            let mut total = 0;
            while j < toks.len() && j - i < MAX_SEQS && total + toks[j].0.len() <= self.max_tokens {
                total += toks[j].0.len();
                j += 1;
            }
            out.extend(self.run_group(&toks[i..j], total)?);
            i = j;
        }
        Ok(out)
    }

    fn run_group(&mut self, group: &[(Vec<llama_token>, bool)], total: usize) -> Result<Vec<Embedding>, EmbedError> {
        unsafe {
            llama_memory_clear(llama_get_memory(self.ctx), true);
            let mut batch = llama_batch_init(total as i32, 0, 1);
            let mut k = 0usize;
            for (seq, (t, _)) in group.iter().enumerate() {
                for (pos, tok) in t.iter().enumerate() {
                    *batch.token.add(k) = *tok;
                    *batch.pos.add(k) = pos as i32;
                    *batch.n_seq_id.add(k) = 1;
                    *(*batch.seq_id.add(k)) = seq as i32;
                    *batch.logits.add(k) = 1;
                    k += 1;
                }
            }
            batch.n_tokens = k as i32;
            let rc = if self.encoder_only { llama_encode(self.ctx, batch) } else { llama_decode(self.ctx, batch) };
            if rc != 0 {
                llama_batch_free(batch);
                return Err(EmbedError::Decode(rc));
            }
            let mut res = Vec::with_capacity(group.len());
            for (seq, (t, tr)) in group.iter().enumerate() {
                let p = llama_get_embeddings_seq(self.ctx, seq as i32);
                if p.is_null() {
                    llama_batch_free(batch);
                    return Err(EmbedError::Decode(-1));
                }
                res.push(Embedding {
                    vector: std::slice::from_raw_parts(p, self.n_embd).to_vec(),
                    n_tokens: t.len(),
                    truncated: *tr,
                });
            }
            llama_batch_free(batch);
            Ok(res)
        }
    }
}

impl Drop for Embedder {
    fn drop(&mut self) {
        unsafe {
            llama_free(self.ctx);
            llama_model_free(self.model);
        }
    }
}
