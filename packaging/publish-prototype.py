"""Publish the completed main build to PeerHatch's rolling GitHub prerelease."""

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


def rename(asset, name):
    api(f"/assets/{asset['id']}", "PATCH", name=name)


def main():
    tag = "prototype"
    sha = os.environ["GITHUB_SHA"]
    root = f"{os.environ['GITHUB_SERVER_URL']}/{os.environ['GITHUB_REPOSITORY']}"
    run = f"{root}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    notes = f"Prototype built from [`{sha[:12]}`]({root}/commit/{sha}). [Build logs]({run})"
    files = sorted(Path("dist").iterdir())
    hashes = {}
    for path in files:
        with path.open("rb") as stream:
            hashes[path.name] = "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()

    # Include unpublished drafts so retrying the first upload reuses the same release.
    releases = json.loads(output("gh", "api", "--paginate", "--slurp",
                                 "repos/{owner}/{repo}/releases?per_page=100"))
    release = next((r for page in releases for r in page if r["tag_name"] == tag), None)
    if release is None:
        release = api("", "POST", tag_name=tag, target_commitish=sha,
                      name="PeerHatch prototype", prerelease=True, draft=True)
    release_path = f"/{release['id']}"
    assets = {a["name"]: a for a in release["assets"]}
    changed = [p for p in files if assets.get(p.name, {}).get("digest") != hashes[p.name]]

    # Stage large uploads before touching the downloads already linked from the release.
    with tempfile.TemporaryDirectory(dir="dist") as temp:
        uploads = []
        for path in changed:
            staged = Path(temp) / ("incoming-" + path.name)
            if assets.get(staged.name, {}).get("digest") != hashes[path.name]:
                os.link(path, staged)
                uploads.append(staged)
        if uploads:
            subprocess.run(["gh", "release", "upload", tag, "--clobber", *uploads], check=True)
    assets = {a["name"]: a for a in api(release_path)["assets"]}
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
        if previous != sha:
            output("git", "push", f"--force-with-lease={ref}:{previous}", "origin", f"{sha}:{ref}")
            moved = True
        api(release_path, "PATCH", name="PeerHatch prototype", body=notes,
            target_commitish=sha, prerelease=True, draft=False, make_latest="false")
    except Exception:
        if moved:
            output("git", "push", f"--force-with-lease={ref}:{sha}", "origin", f"{previous}:{ref}")
        for asset, original in reversed(undo):
            rename(asset, original)
        raise

    for asset in api(release_path)["assets"]:
        name = asset["name"]
        if name not in hashes and name.startswith(("previous-", "incoming-", "peerhatch-")):
            api(f"/assets/{asset['id']}", "DELETE")


if __name__ == "__main__":
    main()
