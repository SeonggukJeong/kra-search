"""사용자가 직접 받은 문서(eval/downloads/)를 평가 세트의 docs/ 로 복사하고 SHA-256을 검증한다.

사용: python3 tools/eval/import_local.py eval/kra
매니페스트의 각 [[doc]] 에 local(eval/downloads 기준 상대 경로)·sha256·id·ext 가 있어야 한다.
"""
import hashlib
import pathlib
import shutil
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parents[2]


def main() -> None:
    eval_dir = REPO / sys.argv[1]
    docs = tomllib.loads((eval_dir / "manifest.toml").read_text("utf-8"))["doc"]
    bad = []
    for d in docs:
        src = REPO / "eval" / "downloads" / d["local"]
        dst = eval_dir / "docs" / f'{d["id"]}.{d["ext"]}'
        if not dst.exists():
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dst)
        if hashlib.sha256(dst.read_bytes()).hexdigest() != d["sha256"]:
            bad.append(d["id"])
    if bad:
        sys.exit(f"SHA-256 mismatch: {', '.join(bad)}")
    print(f"ok: {len(docs)} documents verified")


if __name__ == "__main__":
    main()
