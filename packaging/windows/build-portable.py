"""Build the self-contained Windows x64 release ZIP on a Windows build machine."""

import os
from pathlib import Path
import subprocess
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[2]
TARGET = "x86_64-pc-windows-msvc"


def main():
    build_dir = ROOT / "target" / "windows-user-release"
    # With an explicit target, these flags apply to the Windows binaries only.
    env = dict(os.environ, CARGO_ENCODED_RUSTFLAGS="-C\x1ftarget-feature=+crt-static")
    subprocess.run(
        ["cargo", "build", "--locked", "--release", "--target", TARGET,
         "--target-dir", str(build_dir), "-p", "tunnel-gateway", "-p", "tunnel-cli",
         "-p", "tunnel-gui"],
        cwd=ROOT, env=env, check=True,
    )
    binaries = build_dir / TARGET / "release"
    dist = ROOT / "target" / "dist"
    dist.mkdir(parents=True, exist_ok=True)
    archive = dist / "p2p-tunnel-windows-x64.zip"
    # The ZIP must inherit dist's Windows ACL, including when the builder is elevated.
    temp = tempfile.NamedTemporaryFile(dir=dist, suffix=".zip", delete=False)
    staged = Path(temp.name)
    try:
        with temp, zipfile.ZipFile(temp, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for name in ("tunnel.exe", "tunnel-gateway.exe", "tunnel-gui.exe"):
                bundle.write(binaries / name, name)
            launcher = Path(__file__).with_name("start-gateway.cmd")
            bundle.writestr(launcher.name, launcher.read_text(encoding="ascii").replace("\n", "\r\n"))
        staged.replace(archive)
    finally:
        staged.unlink(missing_ok=True)
    print(archive)


if __name__ == "__main__":
    main()
