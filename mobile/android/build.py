#!/usr/bin/env python3
"""Build the ARM64 paxri APK using the Android SDK."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request
import zipfile

APP = Path(__file__).resolve().parent
OUT = APP.parents[1] / "target/android"
DIST = APP.parents[1] / "dist/android"
APK_BASENAME = "paxri-android-arm64"
TARGET = "aarch64-linux-android"
VERIFIER_HASH = "aa021794230fbc2f0be355999e2cf67398dd563de066a2e17001ffbd0b69101b"
KOTLIN_HASH = "f31cc53f105a7e48c093683bbd5437561d1233920513774b470805641bedbc09"
SCANNER_HASH = "4ab03353127c34e55cbb80fbc9a34031decbcfb1ec5e8f991944d2d494621a33"
ZXING_HASH = "71de5d89341b5fcf5dd89da7f44e84d825d0e084cdf3ec77c9abe26b0f0ceb13"

# Entries are applied in order to the package pinned in Cargo.lock.
PATCH_SERIES = {
    "gpui-mobile": ("hardware-shortcuts.patch", "frame-pacing.patch", "ime-composition.patch",
                    "activity-recreation.patch", "system-appearance.patch"),
    "gpui-pre": ("vertical-scroll.patch", "text-system-lifetime.patch"),
    "gpui-pre-wgpu": ("font-fallback.patch",),
}


def run(*args):
    subprocess.run(args, cwd=APP, check=True)


def download(url, checksum):
    directory = OUT / "dependencies"
    directory.mkdir(exist_ok=True)
    path = directory / url.rsplit("/", 1)[1]
    if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != checksum:
        with urllib.request.urlopen(url, timeout=60) as src:
            data = src.read()
        if hashlib.sha256(data).hexdigest() != checksum:
            raise SystemExit(f"Unexpected SHA-256 for {path.name}")
        path.write_bytes(data)
    return path


def patched_source(package):
    name = package["name"]
    directory = APP / "patches" / name
    patches = [directory / filename for filename in PATCH_SERIES[name]]
    digest = hashlib.sha256(b"\0".join(patch.read_bytes() for patch in patches)).hexdigest()[:16]
    cache = OUT / "dependencies"
    cache.mkdir(exist_ok=True)
    source = cache / f"{package['name']}-{package['version']}-{digest}"
    if not source.is_dir():
        with tempfile.TemporaryDirectory(dir=cache) as tmp:
            staged = Path(tmp) / "gpui"
            shutil.copytree(Path(package["manifest_path"]).parent, staged)
            subprocess.run(["git", "apply", "--no-index", *patches], cwd=staged, check=True,
                           env={**os.environ, "GIT_CEILING_DIRECTORIES": tmp})
            try:
                staged.rename(source)
            except OSError:
                if not source.is_dir():
                    raise
    return str(source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--unsigned-release", action="store_true",
                        help="build an aligned, non-debuggable APK for release signing")
    args = parser.parse_args()
    sdk_path = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk_path:
        raise SystemExit("Set ANDROID_HOME to the Android SDK directory.")
    sdk = Path(sdk_path)
    tools = sdk / "build-tools/35.0.0"
    platform = sdk / "platforms/android-35/android.jar"
    OUT.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("CARGO_TARGET_DIR", str(OUT / "cargo"))
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version=1", "--filter-platform", TARGET], cwd=APP))
    crates = {p["name"]: p for p in metadata["packages"]}
    sources = {name: patched_source(crates[name]) for name in PATCH_SERIES}
    override = "paths = " + json.dumps(list(sources.values()))
    # Align the extracted native library for both 4 KB and 16 KB page devices.
    linker_flags = ["--", "-C", "link-arg=-Wl,-z,max-page-size=16384"]
    profile = "android-release" if args.unsigned_release else "android-dev"
    rustc_args = ["-p", "paxri-android", "--lib", "--locked", "--profile", profile, *linker_flags]
    if os.environ.get("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"):
        run("cargo", "--config", override, "rustc", "--target", TARGET, *rustc_args)
    else:
        run("cargo", "ndk", "-t", "arm64-v8a", "-P", "29", "rustc", "--config", override, *rustc_args)
    native = Path(metadata["target_directory"]) / TARGET / profile / "libpaxri_android.so"
    host = Path(sources["gpui-mobile"]) / "example/android/gradle/app/src/main"
    version = crates["rustls-platform-verifier-android"]["version"]
    aar = download("https://raw.githubusercontent.com/rustls/rustls-platform-verifier/maven-archive/"
                   f"android-release-support/maven/org/rustls/rustls-platform-verifier/{version}/"
                   f"rustls-platform-verifier-{version}.aar", VERIFIER_HASH)
    kotlin = download("https://repo.maven.apache.org/maven2/org/jetbrains/kotlin/kotlin-stdlib/"
                      "2.0.21/kotlin-stdlib-2.0.21.jar", KOTLIN_HASH)
    scanner = download("https://repo.maven.apache.org/maven2/com/journeyapps/zxing-android-embedded/"
                       "4.3.0/zxing-android-embedded-4.3.0.aar", SCANNER_HASH)
    zxing = download("https://repo.maven.apache.org/maven2/com/google/zxing/core/"
                     "3.5.4/core-3.5.4.jar", ZXING_HASH)
    with tempfile.TemporaryDirectory(dir=OUT) as tmp:
        tmp = Path(tmp)
        classes = tmp / "classes"
        classes.mkdir()
        dex = tmp / "dex"
        dex.mkdir()
        with zipfile.ZipFile(aar) as bundle:
            bundle.extract("classes.jar", tmp)
            # The verifier needs the CA-specific CRL network policy as well as its JVM classes.
            bundle.extract("res/xml/network_security_config.xml", tmp)
        with zipfile.ZipFile(scanner) as bundle:
            scanner_classes = bundle.extract("classes.jar", tmp / "scanner")
            for entry in bundle.namelist():
                if entry.startswith("res/"):
                    bundle.extract(entry, tmp)
        generated = tmp / "generated"
        generated.mkdir()
        compiled = tmp / "compiled"
        compiled.mkdir()
        run(tools / "aapt2", "compile", "--dir", tmp / "res", "-o", compiled)
        package = tmp / "base.apk"
        debug_flags = [] if args.unsigned_release else ["--debug-mode"]
        run(tools / "aapt2", "link", *debug_flags, "-I", platform, "--manifest", APP / "AndroidManifest.xml",
            "--java", generated, "--extra-packages", "com.google.zxing.client.android",
            "-o", package, *sorted(compiled.glob("*.flat")))
        java = host / "java/dev/gpui/mobile"
        support = [java / (name + ".java") for name in
                   ("GpuiInputActivity", "GpuiPlatformView", "GpuiCamera", "GpuiVideoPlayer", "GpuiClipboard")]
        classpath = os.pathsep.join(map(str, (platform, scanner_classes, zxing)))
        run("javac", "-proc:none", "-source", "8", "-target", "8", "-cp", classpath, "-d", classes,
            *support, *sorted(generated.rglob("*.java")), *sorted((APP / "java").rglob("*.java")))
        run("java", "-cp", tools / "lib/d8.jar", "com.android.tools.r8.D8", "--min-api", "29",
            "--lib", platform, "--output", dex, *sorted(classes.rglob("*.class")), tmp / "classes.jar",
            kotlin, scanner_classes, zxing)
        with zipfile.ZipFile(package, "a", zipfile.ZIP_DEFLATED) as apk:
            for file in sorted(dex.glob("*.dex")):
                apk.write(file, file.name)
            apk.write(native, "lib/arm64-v8a/" + native.name)
            apk.write(APP.parents[1] / "crates/paxri-ui/fonts/OFL.txt", "assets/licenses/terminal-symbols-ofl.txt")
        aligned = tmp / "aligned.apk"
        run(tools / "zipalign", "-f", "4", package, aligned)
        if args.unsigned_release:
            output = OUT / f"{APK_BASENAME}-unsigned.apk"
            aligned.replace(output)
        else:
            key = OUT / "debug.p12"
            if not key.exists():
                run("keytool", "-genkeypair", "-keystore", key, "-storepass", "android", "-keypass", "android",
                    "-alias", "paxri-debug", "-keyalg", "RSA", "-validity", "10000", "-dname", "CN=paxri Debug")
            DIST.mkdir(parents=True, exist_ok=True)
            output = DIST / f"{APK_BASENAME}.apk"
            with tempfile.TemporaryDirectory(prefix=".sign-", dir=DIST) as staging:
                signed = Path(staging) / output.name
                run(tools / "apksigner", "sign", "--ks", key, "--ks-pass", "pass:android", "--out", signed, aligned)
                run(tools / "apksigner", "verify", signed)
                signed.replace(output)
    print(output)


if __name__ == "__main__":
    main()
