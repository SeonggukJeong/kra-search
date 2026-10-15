//! 데이터 위치(스펙 §5.3): 사용자별 `%LOCALAPPDATA%\KraSearch\{data,keys}`.

use std::path::{Path, PathBuf};

pub struct Home {
    root: PathBuf,
}

impl Home {
    /// Windows: %LOCALAPPDATA%\KraSearch, 그 밖(개발용): $HOME/.kra-search
    pub fn default_root() -> PathBuf {
        if cfg!(windows) {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                return PathBuf::from(local).join("KraSearch");
            }
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join(".kra-search")
    }

    pub fn new(root: impl Into<PathBuf>) -> Home {
        Home { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn keys(&self) -> PathBuf {
        self.root.join("keys")
    }
    pub fn keyword_dir(&self) -> PathBuf {
        self.data().join("tantivy")
    }
    pub fn meta_db(&self) -> PathBuf {
        self.data().join("meta.db")
    }
    pub fn meta_key(&self) -> PathBuf {
        self.keys().join("meta.key")
    }
    /// 색인 단일 인스턴스 잠금 파일. data 밖에 둔다(data 를 지우는 복구 중에도 잠금이 유지되게).
    pub fn lock_file(&self) -> PathBuf {
        self.root.join("kra-search.lock")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout_is_under_root() {
        let h = super::Home::new("/r");
        assert!(h.meta_db().starts_with(h.data()));
        assert!(h.keyword_dir().starts_with(h.data()));
        assert!(h.meta_key().starts_with(h.keys()));
        assert!(!h.keys().starts_with(h.data()), "키는 data 밖(data 를 지워도 남음)");
        assert!(!h.lock_file().starts_with(h.data()), "잠금 파일은 data 밖");
    }
}
