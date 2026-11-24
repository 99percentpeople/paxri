"""Publish the completed main build to paxri's rolling GitHub prerelease."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def api(path, method="GET", **fields):
    args = ["gh", "api", "--method", method, "repos/{owner}/{repo}/releases" + path]
    for key, value in fields.items():
        if isinstance(value, bool):
            args.extend(["-F", f"{key}={str(value).lower()}"])
        else:
            args.extend(["-f", f"{key}={value}"])
    result = output(*args)
    return json.loads(result) if result else None


def release_assets(release_path):
    path = f"repos/{{owner}}/{{repo}}/releases{release_path}/assets?per_page=100"
    pages = json.loads(output("gh", "api", "--paginate", "--slurp", path))
    return {asset["name"]: asset for page in pages for asset in page}


def rename(asset, name):
    api(f"/assets/{asset['id']}", "PATCH", name=name)


def main():
    tag = "rolling"
    sha = os.environ["GITHUB_SHA"]
    root = f"{os.environ['GITHUB_SERVER_URL']}/{os.environ['GITHUB_REPOSITORY']}"
    run = f"{root}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    notes = f"paxri built from [`{sha[:12]}`]({root}/commit/{sha}). [Build logs]({run})"
    dist = Path("dist/release")
    manifest = dist / "SHA256SUMS"
    hashes = {}
    for line in manifest.read_text().splitlines():
        digest, name = line.split("  ", 1)
        hashes[name] = "sha256:" + digest
    hashes[manifest.name] = "sha256:" + hashlib.sha256(manifest.read_bytes()).hexdigest()
    files = sorted(dist.iterdir())
    if {path.name for path in files} != hashes.keys():
        raise ValueError("Release files do not match SHA256SUMS")

    # Include unpublished drafts so retrying the first upload reuses the same release.
    releases = json.loads(output("gh", "api", "--paginate", "--slurp",
                                 "repos/{owner}/{repo}/releases?per_page=100"))
    release = next((r for page in releases for r in page if r["tag_name"] == tag), None)
    if release is None:
        release = api("", "POST", tag_name=tag, target_commitish=sha,
                      name="paxri rolling build", prerelease=True, draft=True)
    release_path = f"/{release['id']}"
    assets = release_assets(release_path)
    changed = [p for p in files if assets.get(p.name, {}).get("digest") != hashes[p.name]]

    # Stage large uploads before touching the downloads already linked from the release.
    with tempfile.TemporaryDirectory(dir=dist) as temp:
        uploads = []
        for path in changed:
            staged = Path(temp) / ("incoming-" + path.name)
            if assets.get(staged.name, {}).get("digest") != hashes[path.name]:
                os.link(path, staged)
                uploads.append(staged)
        if uploads:
            subprocess.run(["gh", "release", "upload", tag, "--clobber", *uploads], check=True)
    assets = release_assets(release_path)
    for path in changed:
        if assets["incoming-" + path.name]["digest"] != hashes[path.name]:
            raise ValueError(f"Uploaded digest mismatch for {path.name}")

    ref = "refs/tags/" + tag
    old_tag = output("git", "ls-remote", "--refs", "origin", ref).split()
    previous = old_tag[0] if old_tag else ""
    undo = []
    moved = False
    try:
        for path in changed:
            name = path.name
            if name in assets:
                old = assets[name]
                rename(old, f"previous-{old['id']}-{name}")
                undo.append((old, name))
            staged = assets["incoming-" + name]
            rename(staged, name)
            undo.append((staged, staged["name"]))
        assets = release_assets(release_path)
        for name, digest in hashes.items():
            if assets.get(name, {}).get("digest") != digest:
                raise ValueError(f"Published asset missing or digest mismatch for {name}")
        if previous != sha:
            output("git", "push", f"--force-with-lease={ref}:{previous}", "origin", f"{sha}:{ref}")
            moved = True
        api(release_path, "PATCH", name="paxri rolling build", body=notes,
            target_commitish=sha, prerelease=True, draft=False, make_latest="false")
    except Exception:
        if moved:
            output("git", "push", f"--force-with-lease={ref}:{sha}", "origin", f"{previous}:{ref}")
        for asset, original in reversed(undo):
            rename(asset, original)
        raise

    # The rolling release contains exactly this build's files after cleanup.
    for name, asset in assets.items():
        if name not in hashes:
            api(f"/assets/{asset['id']}", "DELETE")


if __name__ == "__main__":
    main()
