# Release process

A release is a version number in `Cargo.toml`, a `v*` tag, and the `.deb` that
the tag makes CI build. The steps below run in order.

There is no `CHANGELOG.md`: release notes are generated from the commit log, so
the commit subjects are the user-facing changelog. Write them accordingly.

## Before you start

The version has to be consistent in two places — `Cargo.toml` and the tag. The
`Build Debian package` workflow fails on a mismatch rather than publishing a
package whose name contradicts its tag, but relying on CI to catch it wastes a
run.

## Checklist

### 1. Review what changed since the last tag

```bash
PREV=$(git tag --sort=-creatordate | head -1)
echo "previous: ${PREV:-<none, this is the first release>}"
git log "${PREV:-$(git rev-list --max-parents=0 HEAD)}..HEAD" --oneline
```

This is also what `--generate-notes` will turn into release notes, so it is
worth reading for anything that should not be public or does not read well in
a list of subjects.

### 2. Bump the version

`version` in `Cargo.toml` is the single source of truth. The package, the
artifact name and the release title all derive from it.

```bash
sed -i 's/^version = ".*"/version = "X.Y.Z"/' Cargo.toml
grep '^version' Cargo.toml
```

### 3. Commit and tag

```bash
git add Cargo.toml
git commit -m "Bump version to X.Y.Z"
git tag -a "vX.Y.Z" -m "MihomoManifold X.Y.Z"
git push origin main "vX.Y.Z"
```

The push of the tag is what starts the release. Pushing `main` on its own runs
the tests only — `Build Debian package` triggers on tags and manual dispatch
alone.

### 4. Watch the build

```bash
gh run list --repo "$(gh repo view --json nameWithOwner -q .nameWithOwner)" --limit 5
gh run watch
```

`Run unit tests` must be green before you trust the package. The deb job
re-runs the build on its own, so a broken tree fails there too.

### 5. Check the release

The workflow creates the release if it does not exist, then uploads the package
and its checksum with `--clobber`. Confirm both landed:

```bash
gh release view "vX.Y.Z"
gh release download "vX.Y.Z" --dir /tmp/check
dpkg-deb --info /tmp/check/*.deb
dpkg-deb --contents /tmp/check/*.deb | head
cd /tmp/check && sha256sum -c SHA256SUMS
```

The SHA256SUMS file is generated with paths relative to `result/`, so `sha256sum
-c` has to run from the directory you downloaded into.

## What ships in a release

| Asset | Notes |
|-------|-------|
| `mihomo-manifold_<version>_amd64.deb` | The package. Self-contained: GUI, core, desktop entry, icons. |
| `SHA256SUMS` | Checksum for the above. |

The core version is pinned separately in `packaging/build-deb.sh` and is
reported in the job log, not in the file name. A release that changes the
bundled core should say so in its subject lines, because the artifact name will
not.

## Common failures

| Symptom | Cause | Fix |
|---------|-------|-----|
| `tag vX.Y.Z does not match Cargo.toml version A.B.C` | The two drifted apart | Bump `Cargo.toml`, delete the tag, re-tag and force-push: `git tag -d vX.Y.Z && git tag -a vX.Y.Z -m … && git push origin +refs/tags/vX.Y.Z` |
| Release exists but has no `.deb` | The job failed after `gh release create`, or the tag predates the workflow | Re-run the failed job. If the workflow is not registered on the tag's commit, use [promote-artifact.yml](../.github/workflows/promote-artifact.yml) instead |
| `grep -q … package-contents.txt` failed in the build | The package is missing part of its payload | A real packaging bug. Reproduce locally with `packaging/build-deb.sh`; the same assertions are in `docs/packaging/debian.md` |
| Tag pushed to `upstream` instead of `origin` | Wrong remote | `git push origin +refs/tags/vX.Y.Z` and delete the tag on the other remote. Note that a tag on someone else's repository is visible to them immediately and cannot be un-pushed from your side |
| Release published from the wrong artifact | A manual upload | `gh release upload vX.Y.Z <file> --clobber` overwrites it; re-run the build if the file itself is wrong |

## Publishing to upstream

`origin` is a fork; `cublae/mihomo-manifold` is `upstream`. A tag pushed to
`origin` creates a release on the fork, which is what most users of a fork
want. To propose the work upstream instead:

```bash
gh pr create --repo cublae/mihomo-manifold --head Sam-Fic:main
```

Upstream then runs their own release process, and their tag — not the fork's —
decides what ships. Do not push a release tag to `upstream` unless they ask:
tags on a public repository are visible immediately, and a tag that produces a
Release in someone else's project is not a thing you can quietly undo.
