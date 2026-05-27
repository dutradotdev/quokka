#!/usr/bin/env bash
# Cut a new quokka release.
#
#   ./scripts/release.sh 0.2.2
#
# Bumps Cargo.toml + Cargo.lock, runs tests, commits, pushes main, then
# pushes the matching v<version> tag — which triggers the Release
# workflow (.github/workflows/release.yml) to build artifacts and update
# the homebrew tap.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <version>" >&2
  echo "  e.g. $0 0.2.2" >&2
  exit 2
fi

VERSION="$1"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]]; then
  echo "error: '$VERSION' is not a valid semver (X.Y.Z or X.Y.Z-suffix)" >&2
  exit 2
fi

TAG="v$VERSION"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "error: working tree has uncommitted changes. Commit or stash first." >&2
  git status --short >&2
  exit 1
fi

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$BRANCH" != "main" ]]; then
  echo "error: must release from main (currently on '$BRANCH')." >&2
  exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "error: tag $TAG already exists locally." >&2
  exit 1
fi

if git ls-remote --tags origin "$TAG" | grep -q "$TAG"; then
  echo "error: tag $TAG already exists on origin." >&2
  exit 1
fi

CURRENT="$(grep -E '^version = ' Cargo.toml | head -1 | sed -E 's/version = "(.*)"/\1/')"
echo "current version: $CURRENT"
echo "new version:     $VERSION"
echo "tag:             $TAG"
echo
read -r -p "proceed? [y/N] " ans
case "$ans" in
  y|Y|yes) ;;
  *) echo "aborted."; exit 0 ;;
esac

sed -i.bak -E "s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml
rm Cargo.toml.bak
cargo build --quiet

git add Cargo.toml Cargo.lock
git commit -m "chore: release $TAG"

echo
echo "about to push main and tag $TAG (this triggers the Release workflow)."
read -r -p "push now? [y/N] " ans
case "$ans" in
  y|Y|yes) ;;
  *)
    echo "commit kept locally; not pushed. To push later:"
    echo "  git push origin main && git tag $TAG && git push origin $TAG"
    exit 0
    ;;
esac

git push origin main
git tag "$TAG"
git push origin "$TAG"

echo
echo "released $TAG. Watch:"
echo "  https://github.com/dutradotdev/quokka/actions"
