"""Build the self-contained Windows x64 release ZIP on a Windows build machine."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from windows_conpty import runtime_files
TARGET = "x86_64-pc-windows-msvc"


def main():
    build_dir = ROOT / "target" / "windows-user-release"
    # With an explicit target, these flags apply to the Windows binaries only.
    env = dict(os.environ, CARGO_ENCODED_RUSTFLAGS="-C\x1ftarget-feature=+crt-static")
    subprocess.run(
        ["cargo", "--config", "packaging/distribution.toml", "build", "--locked",
         "--profile", "distribution", "--target", TARGET,
         "--target-dir", str(build_dir), "-p", "peerhatch-gateway", "-p", "peerhatch-cli",
         "-p", "peerhatch-gui"],
        cwd=ROOT, env=env, check=True,
    )
    binaries = build_dir / TARGET / "distribution"
    for name in ("peerhatch.exe", "peerhatch-gateway.exe", "peerhatch-gui.exe"):
        subprocess.run([str(binaries / name), "--version"], check=True)
    dist = ROOT / "target" / "dist"
    dist.mkdir(parents=True, exist_ok=True)
    archive = dist / "peerhatch-windows-x64.zip"
    # The ZIP must inherit dist's Windows ACL, including when the builder is elevated.
    temp = tempfile.NamedTemporaryFile(dir=dist, suffix=".zip", delete=False)
    staged = Path(temp.name)
    try:
        with temp, zipfile.ZipFile(temp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
            for name in ("peerhatch.exe", "peerhatch-gateway.exe", "peerhatch-gui.exe"):
                bundle.write(binaries / name, name)
            for name, data in runtime_files(binaries / "peerhatch-gateway.exe", ROOT / "target/conpty").items():
                bundle.writestr(name, data)
            bundle.write(ROOT / "packaging/windows/ConPTY-LICENSE.txt", "ConPTY-LICENSE.txt")
            bundle.write(ROOT / "README.md", "README.md")
            bundle.write(ROOT / "docs/installation.md", "installation.md")
        staged.replace(archive)
    finally:
        staged.unlink(missing_ok=True)
    print(archive)


if __name__ == "__main__":
    main()
