use std::path::{Path, PathBuf};
use std::{env, process::Command};

/// patches/*.patch 를 vendor/llama.cpp 에 한 번만 적용한다(이미 적용돼 있으면 건너뜀).
fn apply_patches(src: &Path, dir: &Path) {
    let mut patches: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "patch")).collect())
        .unwrap_or_default();
    patches.sort();
    for p in patches {
        println!("cargo:rerun-if-changed={}", p.display());
        let git = |args: &[&str]| {
            Command::new("git").arg("-C").arg(src).args(args).arg("--ignore-whitespace").arg(&p).status().map(|s| s.success()).unwrap_or(false)
        };
        if git(&["apply", "--reverse", "--check"]) {
            continue;
        }
        assert!(git(&["apply"]), "failed to apply {}", p.display());
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("vendor/llama.cpp");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=patches");
    println!("cargo:rerun-if-env-changed=KRA_LLAMA_COMPILER");
    apply_patches(&src, &manifest.join("patches"));
    println!("cargo:rerun-if-changed=vendor/llama.cpp/include/llama.h");

    let mut cfg = cmake::Config::new(&src);
    for (k, v) in [
        ("LLAMA_CURL", "OFF"),
        ("LLAMA_BUILD_TESTS", "OFF"),
        ("LLAMA_BUILD_EXAMPLES", "OFF"),
        ("LLAMA_BUILD_TOOLS", "OFF"),
        ("LLAMA_BUILD_SERVER", "OFF"),
        ("LLAMA_BUILD_APP", "OFF"),
        ("LLAMA_BUILD_COMMON", "OFF"),
        ("GGML_OPENMP", "OFF"),
        ("GGML_NATIVE", "OFF"),
        ("GGML_METAL", "OFF"),
        ("GGML_BLAS", "OFF"),
    ] {
        cfg.define(k, v);
    }
    cfg.profile("Release");
    if os == "windows" {
        cfg.define("BUILD_SHARED_LIBS", "ON")
            .define("GGML_BACKEND_DL", "ON")
            .define("GGML_CPU_ALL_VARIANTS", "ON")
            // DLL마다 정적 CRT를 넣으면 DLL 사이에서 CRT 자원(힙·FILE*)이 섞여 fast-fail(0xC0000409)로 죽는다.
            // → llama/ggml DLL은 동적 CRT(/MD). 배포 시 VC++ 런타임 DLL을 exe 옆에 함께 둔다.
            .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL");
        // cmake 크레이트는 CMAKE_<LANG>_FLAGS_RELEASE 를 cc 크레이트 값(-nologo -MT -Brepro)으로 덮어써
        // 최적화(/O2)가 빠진다(실측: 초당 29토큰). 직접 지정하면 cmake 크레이트가 덮어쓰지 않는다.
        if env::var("KRA_LLAMA_COMPILER").as_deref() == Ok("clang") {
            // 실험용: upstream 릴리즈처럼 Ninja + clang/clang++
            cfg.generator("Ninja")
                .define("CMAKE_C_COMPILER", "clang")
                .define("CMAKE_CXX_COMPILER", "clang++")
                .define("CMAKE_C_FLAGS", "")
                .define("CMAKE_CXX_FLAGS", "")
                .define("CMAKE_C_FLAGS_RELEASE", "-O3 -DNDEBUG")
                .define("CMAKE_CXX_FLAGS_RELEASE", "-O3 -DNDEBUG");
        } else {
            cfg.define("CMAKE_C_FLAGS", "/nologo /W0 /utf-8")
                .define("CMAKE_CXX_FLAGS", "/nologo /W0 /utf-8 /EHsc")
                .define("CMAKE_C_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG")
                .define("CMAKE_CXX_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG");
        }
    } else {
        cfg.define("BUILD_SHARED_LIBS", "OFF");
    }
    let dst = cfg.build();
    // 회귀 방지: llama.cpp 가 최적화 없이 빌드되면(실제로 Windows 에서 17배 느려졌던 문제) 빌드를 실패시킨다.
    let cache = std::fs::read_to_string(dst.join("build").join("CMakeCache.txt")).unwrap_or_default();
    let rel = cache.lines().find(|l| l.starts_with("CMAKE_C_FLAGS_RELEASE:")).unwrap_or("");
    assert!(
        ["/O2", "-O2", "-O3"].iter().any(|o| rel.contains(o)),
        "llama.cpp would be compiled without optimization: {rel}"
    );
    for d in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(d).display());
    }

    if os == "windows" {
        for l in ["llama", "ggml", "ggml-base"] {
            println!("cargo:rustc-link-lib=dylib={l}");
        }
        println!("cargo:dll_dir={}", dst.join("bin").display());
    } else {
        for l in ["llama", "ggml", "ggml-cpu", "ggml-base"] {
            println!("cargo:rustc-link-lib=static={l}");
        }
        if os == "macos" {
            println!("cargo:rustc-link-lib=c++");
            println!("cargo:rustc-link-lib=framework=Accelerate");
        } else {
            println!("cargo:rustc-link-lib=stdc++");
        }
    }

    bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_arg(format!("-I{}", src.join("include").display()))
        .clang_arg(format!("-I{}", src.join("ggml/include").display()))
        .allowlist_function("llama_.*")
        .allowlist_function("ggml_backend_load_all_from_path")
        .allowlist_type("llama_.*")
        .allowlist_var("LLAMA_.*")
        .generate()
        .expect("bindgen failed")
        .write_to_file(PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs"))
        .unwrap();
}
