#!/bin/sh
# Build this host's native release archive. There is no CI: run it on a
# matching host for each target, then publish the archives and the image by
# hand (see docs/packaging.md).
set -eu

usage() {
	cat <<'EOF'
usage: scripts/release.sh --version X.Y.Z [options]

Builds this host's native archive: web/dist, the release binary, and
dist/otterscope_<version>_<artifact>.tar.gz (.zip on Windows).

  --version X.Y.Z   release version without the leading "v" (required)
  --target TRIPLE   cargo target triple (default: rustc's host triple)
  --artifact NAME   archive suffix (default: derived from the target)
  --dist DIR        output directory (default: dist)
  --dry-run         check the tree and version, print the plan, build nothing
  -h, --help        show this help

The script refuses a dirty tree and a rust/Cargo.toml version that differs
from --version. It pushes nothing: the GitHub release and GHCR commands are
printed at the end.
EOF
}

die() {
	printf 'release: %s\n' "$*" >&2
	exit 1
}

say() {
	printf '==> %s\n' "$*"
}

version=
target=
artifact=
dist=dist
dry_run=0

while [ "$#" -gt 0 ]; do
	case "$1" in
		--version)
			[ "$#" -ge 2 ] || { usage >&2; exit 2; }
			version=$2
			shift 2
			;;
		--target)
			[ "$#" -ge 2 ] || { usage >&2; exit 2; }
			target=$2
			shift 2
			;;
		--artifact)
			[ "$#" -ge 2 ] || { usage >&2; exit 2; }
			artifact=$2
			shift 2
			;;
		--dist)
			[ "$#" -ge 2 ] || { usage >&2; exit 2; }
			dist=$2
			shift 2
			;;
		--dry-run)
			dry_run=1
			shift
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			printf 'release: unknown argument: %s\n' "$1" >&2
			usage >&2
			exit 2
			;;
	esac
done

[ -n "$version" ] || { usage >&2; exit 2; }
case "$version" in
	v*) die "pass the version without the leading v: $version" ;;
esac
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' \
	|| die "not a version like 1.2.3 or 1.2.3-rc.1: $version"

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

[ -f rust/Cargo.toml ] || die "rust/Cargo.toml not found; is this the repository?"
command -v git >/dev/null 2>&1 || die "git is required"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not inside a git checkout"
command -v rustc >/dev/null 2>&1 || die "rustc is required (put cargo/rustup on PATH)"
command -v cargo >/dev/null 2>&1 || die "cargo is required (put cargo/rustup on PATH)"
command -v bun >/dev/null 2>&1 || die "bun is required (web/ build)"

if [ -n "$(git status --porcelain)" ]; then
	printf 'release: the working tree is dirty; commit or stash before releasing:\n' >&2
	git status --short >&2
	exit 1
fi

workspace_version=$(awk -F'"' '
	/^\[workspace\.package\]/ { in_section = 1; next }
	/^\[/ { in_section = 0 }
	in_section && /^version[[:space:]]*=/ { print $2; exit }
' rust/Cargo.toml)
[ -n "$workspace_version" ] || die "cannot read [workspace.package] version from rust/Cargo.toml"
[ "$workspace_version" = "$version" ] || die "rust/Cargo.toml says version $workspace_version, not $version; bump [workspace.package] version in the release commit"

[ -n "$target" ] || target=$(rustc -vV | sed -n 's/^host: //p')
[ -n "$target" ] || die "cannot determine the host target triple; pass --target"

if [ -z "$artifact" ]; then
	case "$target" in
		x86_64-unknown-linux-gnu) artifact=linux-amd64 ;;
		aarch64-unknown-linux-gnu) artifact=linux-arm64 ;;
		aarch64-apple-darwin) artifact=darwin-arm64 ;;
		x86_64-apple-darwin) artifact=darwin-amd64 ;;
		x86_64-pc-windows-msvc) artifact=windows-amd64 ;;
		*) die "no archive name for target $target; pass --artifact" ;;
	esac
fi

name="otterscope_${version}_${artifact}"
binary="rust/target/${target}/release/otterscope"
case "$target" in
	*windows*)
		binary="${binary}.exe"
		archive="${name}.zip"
		pack="(cd ${dist}/.staging && zip -qr ../${archive} ${name})"
		;;
	*)
		archive="${name}.tar.gz"
		pack="tar -czf ${dist}/${archive} -C ${dist}/.staging ${name}"
		;;
esac

publish_hints() {
	cat <<EOF

publish (manual, unsigned):
  gh release create "v${version}" ${dist}/* --title "v${version}" --generate-notes
  # if the release exists: gh release upload "v${version}" ${dist}/* --clobber

container image (manual; needs docker buildx and ghcr.io write access):
  echo "\$GHCR_TOKEN" | docker login ghcr.io -u <github-user> --password-stdin
  docker buildx create --name otterscope --use   # once per machine
  docker buildx build \\
    --platform linux/amd64,linux/arm64 \\
    --build-arg version=${version} \\
    -t ghcr.io/otterscope/otterscope:${version} \\
    -t ghcr.io/otterscope/otterscope:latest \\
    --push .

Build the other three archives on their matching hosts before publishing;
docs/packaging.md has the Docker path for the two Linux targets.
EOF
}

if [ "$dry_run" -eq 1 ]; then
	say "dry run: tree is clean and the version matches; nothing built"
	cat <<EOF
plan:
  version:  ${version}
  target:   ${target}
  artifact: ${artifact}
  archive:  ${dist}/${archive}
  steps:
    1. (cd web && bun install --frozen-lockfile && bun run build)
    2. (cd rust && cargo build --release --target ${target} --locked)
    3. stage ${binary##*/}, LICENSE and README.md under ${name}/
    4. ${pack}
EOF
	publish_hints
	exit 0
fi

mkdir -p "$dist"
staging="$dist/.staging/$name"
cleanup() {
	rm -rf "$staging"
	rmdir "$dist/.staging" 2>/dev/null || true
}
trap cleanup 0 1 2 3 15

say "building web"
(cd web && bun install --frozen-lockfile && bun run build)

say "building rust for $target"
(cd rust && cargo build --release --target "$target" --locked)
[ -f "$binary" ] || die "expected binary not found: $binary"

say "staging $name"
mkdir -p "$staging"
cp "$binary" "$staging/"
cp LICENSE README.md "$staging/"

case "$archive" in
	*.zip)
		command -v zip >/dev/null 2>&1 || die "zip is required for a Windows archive"
		(cd "$dist/.staging" && zip -qr "../$archive" "$name")
		;;
	*)
		tar -czf "$dist/$archive" -C "$dist/.staging" "$name"
		;;
esac

say "wrote $dist/$archive"
publish_hints
