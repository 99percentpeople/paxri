#!/usr/bin/env python3
"""Build and sign the ARM64 PeerHatch development APK using the Android SDK."""
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
TARGET = "aarch64-linux-android"
VERIFIER_HASH = "aa021794230fbc2f0be355999e2cf67398dd563de066a2e17001ffbd0b69101b"
KOTLIN_HASH = "f31cc53f105a7e48c093683bbd5437561d1233920513774b470805641bedbc09"
SCANNER_HASH = "4ab03353127c34e55cbb80fbc9a34031decbcfb1ec5e8f991944d2d494621a33"
ZXING_HASH = "71de5d89341b5fcf5dd89da7f44e84d825d0e084cdf3ec77c9abe26b0f0ceb13"


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


def vertical_scroll_source(package):
    # The pinned GPUI recognizer discards Y when a pan starts mostly horizontally.
    # Apply the Android client's vertical viewport policy without editing Cargo's registry cache.
    patch = APP / "gpui-vertical-scroll.patch"
    digest = hashlib.sha256(patch.read_bytes()).hexdigest()[:16]
    cache = OUT / "dependencies"
    cache.mkdir(exist_ok=True)
    source = cache / f"gpui-pre-{package['version']}-{digest}"
    if not source.is_dir():
        with tempfile.TemporaryDirectory(dir=cache) as tmp:
            staged = Path(tmp) / "gpui"
            shutil.copytree(Path(package["manifest_path"]).parent, staged)
            subprocess.run(["git", "apply", "--no-index", str(patch)], cwd=staged, check=True,
                           env={**os.environ, "GIT_CEILING_DIRECTORIES": tmp})
            try:
                staged.rename(source)
            except OSError:
                if not source.is_dir():
                    raise
    return "paths = " + json.dumps([str(source)])


def main():
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
    override = vertical_scroll_source(crates["gpui-pre"])
    # Align the extracted native library for both 4 KB and 16 KB page devices.
    linker_flags = ["--", "-C", "link-arg=-Wl,-z,max-page-size=16384"]
    if os.environ.get("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"):
        run("cargo", "--config", override, "rustc", "--lib", "--locked", "--target", TARGET, *linker_flags)
    else:
        run("cargo", "ndk", "-t", "arm64-v8a", "-P", "29", "rustc", "--config", override,
            "--lib", "--locked", *linker_flags)
    native = Path(metadata["target_directory"]) / TARGET / "debug/libpeerhatch_android.so"
    host = Path(crates["gpui-mobile"]["manifest_path"]).parent / "example/android/gradle/app/src/main"
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
    key = OUT / "debug.p12"
    if not key.exists():
        run("keytool", "-genkeypair", "-keystore", key, "-storepass", "android", "-keypass", "android",
            "-alias", "peerhatch-debug", "-keyalg", "RSA", "-validity", "10000", "-dname", "CN=PeerHatch Debug")
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
        run(tools / "aapt2", "link", "--debug-mode", "-I", platform, "--manifest", APP / "AndroidManifest.xml",
            "--java", generated, "--extra-packages", "com.google.zxing.client.android",
            "-o", package, *sorted(compiled.glob("*.flat")))
        java = host / "java/dev/gpui/mobile"
        # Expose committed edits on the Android UI thread so toolbar releases and IME
        # commits share an event order. Keep the pinned GPUI proxy's batch/composition logic.
        host_activity = tmp / "GpuiInputActivity.java"
        source = (java / host_activity.name).read_text()
        body, declaration, tail = source.partition("    private static native void nativeIme(")
        if not declaration:
            raise RuntimeError("The GPUI Android input proxy has changed")
        hook = """    protected void deliverIme(long session, int kind, String text, int start, int end) {
        nativeIme(session, kind, text, start, end);
    }
"""
        host_activity.write_text(body.replace("nativeIme(", "deliverIme(") + hook + declaration + tail)
        support = [host_activity, *(java / (name + ".java") for name in
                   ("GpuiPlatformView", "GpuiCamera", "GpuiVideoPlayer"))]
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
            apk.write(host / "assets/fonts/NotoColorEmoji.ttf", "assets/fonts/NotoColorEmoji.ttf")
        aligned = tmp / "aligned.apk"
        run(tools / "zipalign", "-f", "4", package, aligned)
        signed = tmp / "PeerHatch.apk"
        run(tools / "apksigner", "sign", "--ks", key, "--ks-pass", "pass:android", "--out", signed, aligned)
        run(tools / "apksigner", "verify", signed)
        shutil.move(signed, OUT / "PeerHatch.apk")
    print(OUT / "PeerHatch.apk")


if __name__ == "__main__":
    main()
