#!/usr/bin/env bash
#
# Build a .deb around the release binary.
#
# Runs the same way locally and in CI, so a packaging bug is reproducible
# without pushing a tag:
#
#   cargo build --release
#   packaging/build-deb.sh
#
# The result is self-contained: the GUI, the mihomo core, the desktop entry and
# the icons. Installing grants the core the capabilities TUN needs, and a
# wrapper in /usr/bin points the GUI at that core.

set -euo pipefail

PKG=mihomo-manifold
# The application ID, which is what the desktop entry and the icon theme key
# off. It is not the package name, and conflating the two is why the icon and
# the .desktop file are looked up separately below.
APP_ID=io.github.cublae.MihomoManifold
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OUT=${OUT:-$ROOT/result}
BUILD=$ROOT/.build
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ---------------------------------------------------------------- inputs

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
[ -n "$VERSION" ] || die "no version in Cargo.toml"

ARCH=${ARCH:-$(dpkg --print-architecture)}
LIBDIR=/usr/lib/$PKG

# Pinned separately from the app: a new core is a security decision, not
# something a GUI rebuild should silently pull in.
MIHOMO_VERSION=${MIHOMO_VERSION:-v1.19.31}
MIHOMO_TAG=${MIHOMO_TAG:-$MIHOMO_VERSION}

case "$ARCH" in
    amd64) MIHOMO_ASSET="mihomo-linux-amd64-v1-${MIHOMO_VERSION}.gz" ;;
    arm64) MIHOMO_ASSET="mihomo-linux-arm64-v1-${MIHOMO_VERSION}.gz" ;;
    *)     die "no mihomo asset mapped for '$ARCH' — add a case to build-deb.sh" ;;
esac
MIHOMO_URL=${MIHOMO_URL:-https://github.com/MetaCubeX/mihomo/releases/download/$MIHOMO_TAG/$MIHOMO_ASSET}

GUI_SRC=$ROOT/target/release/$PKG
[ -x "$GUI_SRC" ] || die "$GUI_SRC missing — run: cargo build --release"

# ---------------------------------------------------------------- core

say "core $MIHOMO_VERSION for $ARCH"
mkdir -p "$BUILD"
if [ -n "${MIHOMO_BIN:-}" ]; then
    # Escape hatch for offline builds and for testing the packaging without
    # spending 20 MB of bandwidth on the same core twice.
    say "using existing core $MIHOMO_BIN"
    install -m 755 "$MIHOMO_BIN" "$BUILD/mihomo"
elif [ ! -x "$BUILD/mihomo" ]; then
    have curl || die "curl is required"
    say "downloading $MIHOMO_URL"
    curl -fsSL --retry 3 -o "$BUILD/mihomo.gz" "$MIHOMO_URL"
    # mihomo ships a plain executable, not an archive, inside a .gz.
    gzip -dc "$BUILD/mihomo.gz" > "$BUILD/mihomo"
    chmod 755 "$BUILD/mihomo"
    rm -f "$BUILD/mihomo.gz"
fi
"$BUILD/mihomo" -v | head -1 | sed 's/^/    /'

# ---------------------------------------------------------------- icons

ICON_SRC=$ROOT/data/icons/$APP_ID.svg
[ -f "$ICON_SRC" ] || die "icon missing: $ICON_SRC"
say "rendering icons"
# rsvg-convert keeps the geometry honest at small sizes; ImageMagick's built-in
# SVG reader drops everything after the first element.
if have rsvg-convert; then
    render() { rsvg-convert -w "$1" -h "$1" -o "$2" "$ICON_SRC"; }
elif python3 -c 'import cairosvg' 2>/dev/null; then
    render() {
        python3 - "$1" "$2" "$ICON_SRC" <<-'PY'
	import sys, cairosvg
	size, out, src = int(sys.argv[1]), sys.argv[2], sys.argv[3]
	cairosvg.svg2png(url=src, write_to=out, output_width=size, output_height=size)
	PY
    }
else
    die "need rsvg-convert (librsvg2-bin) or python3-cairosvg to render the icon"
fi
for size in 16 24 32 48 64 128 256 512; do
    mkdir -p "$BUILD/icons"
    render "$size" "$BUILD/icons/$size.png"
done

# ---------------------------------------------------------------- layout

DEST=$STAGE/${PKG}_${VERSION}_${ARCH}
install -d "$DEST/DEBIAN" "$DEST/usr/bin" "$DEST$LIBDIR" \
           "$DEST/usr/share/applications" "$DEST/usr/share/doc/$PKG"

install -m 755 "$GUI_SRC" "$DEST$LIBDIR/$PKG"
install -m 755 "$BUILD/mihomo" "$DEST$LIBDIR/mihomo"

# A wrapper rather than a code change: MIHOMO_MANIFOLD_CORE is already the
# documented override, the app checks its own config first, and `exec` means
# this costs no extra process.
cat > "$DEST/usr/bin/$PKG" <<WRAPPER
#!/bin/sh
# Point the GUI at the core shipped alongside it.
: "\${MIHOMO_MANIFOLD_CORE:=$LIBDIR/mihomo}"
export MIHOMO_MANIFOLD_CORE
exec $LIBDIR/$PKG "\$@"
WRAPPER
chmod 755 "$DEST/usr/bin/$PKG"

install -m 644 "$ROOT/data/$APP_ID.desktop" "$DEST/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$ICON_SRC" "$DEST/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 "$BUILD/icons/$size.png" \
        "$DEST/usr/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done
install -m 644 "$ROOT/LICENSE" "$DEST/usr/share/doc/$PKG/copyright"

# ---------------------------------------------------------------- control

# dpkg-shlibdeps reads DT_NEEDED and resolves each soname through the package
# database, which `dpkg -S` alone cannot do: several of these libraries are
# shipped by more than one package (deepin-wine-runtime bundles its own glib,
# appimagelauncher its own libssl) and a plain text match picks the wrong one.
#
# Run against the staged binary so the tool sees the path it will really live
# at, and so it emits DT_NEEDED rather than the whole transitive closure.
runtime_depends() {
    local shlibdir
    shlibdir=$(mktemp -d)
    mkdir -p "$shlibdir/debian"
    printf 'Source: %s\nSection: net\nMaintainer: build\nStandards-Version: 4.7.0\n' "$PKG" \
        > "$shlibdir/debian/control"
    # Same layout the package installs into, so the analysis matches it.
    mkdir -p "$shlibdir/$LIBDIR"
    cp "$DEST$LIBDIR/$PKG" "$shlibdir/$LIBDIR/$PKG"

    local out
    out=$(cd "$shlibdir" && dpkg-shlibdeps -O "./$LIBDIR/$PKG" 2>/dev/null \
        | sed -n 's/^shlibs:Depends=//p')
    rm -rf "$shlibdir"
    [ -n "$out" ] || return 1
    printf '%s\n' "$out" | tr ',' '\n' | sed 's/^ *//; s/ *$//' | grep -v '^$'
}

# dpkg-shlibdeps reports the version the build machine happens to carry. For
# these two the build-time API matters more: the v4_18 and v1_5 features are
# compiled against and fail at load time on anything older, regardless of what
# happens to be installed here.
raise_floor() {
    sed -E "s#^$1 \(>= [^)]*\)#$1 (>= $2)#"
}

if ! DEPS=$(runtime_depends); then
    die "dpkg-shlibdeps failed — install dpkg-dev (Debian: build-essential)"
fi
DEPS=$(printf '%s\n' "$DEPS" \
    | raise_floor libgtk-4-1 4.18 \
    | raise_floor libadwaita-1-0 1.5 \
    | paste -sd, -)

cat > "$DEST/DEBIAN/control" <<CONTROL
Package: $PKG
Version: $VERSION
Section: net
Priority: optional
Architecture: $ARCH
Maintainer: MihomoManifold maintainers <https://github.com/cublae/$PKG/issues>
Installed-Size: $(du -ks "$DEST/usr" | cut -f1)
Depends: $DEPS
Homepage: https://github.com/cublae/$PKG
Description: GTK4 front-end for the mihomo proxy core
 Subscriptions that carry a device identifier, split routing you control, and
 a core the GUI owns rather than hides. Ships the mihomo core it drives.
CONTROL

install -m 755 "$ROOT/packaging/debian/postinst" "$DEST/DEBIAN/postinst"
install -m 755 "$ROOT/packaging/debian/postrm" "$DEST/DEBIAN/postrm"

# ---------------------------------------------------------------- build

mkdir -p "$OUT"
DEB=$OUT/${PKG}_${VERSION}_${ARCH}.deb
rm -f "$DEB"
dpkg-deb --root-owner-group --build "$DEST" "$DEB" >/dev/null

say "built $DEB ($(du -h "$DEB" | cut -f1))"
if have lintian; then
    say "lintian"
    lintian --no-tag-display-limit "$DEB" 2>&1 | sed 's/^/    /' || true
fi
