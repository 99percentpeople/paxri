"""Build the self-contained Windows x64 release ZIP on a Windows build machine."""

import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from windows_conpty import runtime_files
TARGET = "x86_64-pc-windows-msvc"
LINK_RE = re.compile(r"\]\(([^)]+)\)")


def rewrite_release_doc(doc: str, path: Path) -> bytes:
    server = os.environ.get("GITHUB_SERVER_URL") or "https://github.com"
    repository = os.environ.get("GITHUB_REPOSITORY") or "99percentpeople/paxri"
    repo = f"{server.rstrip('/')}/{repository.strip('/')}"

    def replace(match: re.Match[str]) -> str:
        target = match.group(1)
        if target.startswith(("http://", "https://", "mailto:", "#")):
            return match.group(0)
        if doc == "readme":
            if target == "docs/installation.md" or target.startswith("docs/installation.md#"):
                target = target[len("docs/"):]
            elif target in ("README.md", "installation.md") or target.startswith(("README.md#", "installation.md#")):
                pass
            elif target.startswith("../../"):
                target = f"{repo}/{target[len('../../'):]}"
            else:
                target = f"{repo}/blob/main/{target}"
        elif doc == "installation":
            if target == "../README.md" or target.startswith("../README.md#"):
                target = target[len("../"):]
            elif target in ("README.md", "installation.md") or target.startswith(("README.md#", "installation.md#")):
                pass
            elif target.startswith("../../../"):
                target = f"{repo}/{target[len('../../../'):]}"
            elif target.startswith("../"):
                target = f"{repo}/blob/main/{target[len('../'):]}"
            else:
                target = f"{repo}/blob/main/docs/{target}"
        return f"]({target})"

    in_code = False
    lines = []
    for line in path.read_text(encoding="utf-8").splitlines(keepends=True):
        if line.startswith("```"):
            in_code = not in_code
            lines.append(line)
        elif in_code:
            lines.append(line)
        else:
            lines.append(LINK_RE.sub(replace, line))
    return "".join(lines).encode("utf-8")


def main():
    build_dir = ROOT / (os.environ.get("CARGO_TARGET_DIR") or "target/windows-user-release")
    # With an explicit target, these flags apply to the Windows binaries only.
    env = dict(os.environ, CARGO_ENCODED_RUSTFLAGS="-C\x1ftarget-feature=+crt-static")
    subprocess.run(
        ["cargo", "--config", "packaging/distribution.toml", "build", "--locked",
         "--profile", "distribution", "--target", TARGET,
         "--target-dir", str(build_dir), "-p", "paxri-gateway", "-p", "paxri-cli",
         "-p", "paxri-gui"],
        cwd=ROOT, env=env, check=True,
    )
    binaries = build_dir / TARGET / "distribution"
    for name in ("paxri.exe", "paxri-gateway.exe", "paxri-gui.exe"):
        subprocess.run([str(binaries / name), "--version"], check=True)
    dist = ROOT / "dist/native"
    dist.mkdir(parents=True, exist_ok=True)
    archive = dist / "paxri-windows-x64.zip"
    # The ZIP must inherit dist's Windows ACL, including when the builder is elevated.
    temp = tempfile.NamedTemporaryFile(dir=dist, suffix=".zip", delete=False)
    staged = Path(temp.name)
    try:
        with temp, zipfile.ZipFile(temp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
            for name in ("paxri.exe", "paxri-gateway.exe", "paxri-gui.exe"):
                bundle.write(binaries / name, name)
            for name, data in runtime_files(binaries / "paxri-gateway.exe", ROOT / "target/conpty").items():
                bundle.writestr(name, data)
            bundle.write(ROOT / "packaging/windows/ConPTY-LICENSE.txt", "licenses/conpty.txt")
            bundle.write(ROOT / "crates/paxri-ui/fonts/OFL.txt", "licenses/terminal-symbols-ofl.txt")
            bundle.writestr("README.md", rewrite_release_doc("readme", ROOT / "README.md"))
            bundle.writestr("installation.md", rewrite_release_doc("installation", ROOT / "docs/installation.md"))
        staged.replace(archive)
    finally:
        staged.unlink(missing_ok=True)
    print(archive)


if __name__ == "__main__":
    main()
