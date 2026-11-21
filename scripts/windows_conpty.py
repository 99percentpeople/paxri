"""Read the matching console host and DLL from Microsoft's pinned ConPTY package."""
import hashlib
import io
from pathlib import Path
import tempfile
import urllib.request
import zipfile


VERSION = "1.25.261109001"
SHA256 = "3d77f535df848e44225c2a8912ac844fecb23f3057732b639f3034b587496f09"
FILES = ("conpty.dll", "OpenConsole.exe")


def runtime_files(gateway, cache):
    with gateway.open("rb") as image:
        image.seek(0x3c)
        image.seek(int.from_bytes(image.read(4), "little"))
        if image.read(4) != b"PE\0\0":
            raise ValueError(f"Invalid Windows gateway executable: {gateway}")
        arch = {0x8664: "x64", 0xaa64: "arm64", 0x14c: "x86"}[int.from_bytes(image.read(2), "little")]

    name = f"microsoft.windows.console.conpty.{VERSION}.nupkg"
    package = cache / name
    data = package.read_bytes() if package.exists() else b""
    if hashlib.sha256(data).hexdigest() != SHA256:
        url = f"https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/{VERSION}/{name}"
        with urllib.request.urlopen(url, timeout=45) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != SHA256:
            raise ValueError(f"Unexpected checksum for {name}")
        cache.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=cache, delete=False) as tmp:
            tmp.write(data)
        Path(tmp.name).replace(package)
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        return {"conpty.dll": archive.read(f"runtimes/win-{arch}/native/conpty.dll"),
                "OpenConsole.exe": archive.read(f"build/native/runtimes/{arch}/OpenConsole.exe")}
