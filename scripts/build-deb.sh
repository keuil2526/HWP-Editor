#!/usr/bin/env bash
# Build dist/hwpedit_<version>_<arch>.deb from a release build.
# Usage: scripts/build-deb.sh
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f vendor/rhwp/Cargo.toml ] || { echo "vendor/rhwp 가 없습니다. README의 rhwp 받기 단계를 먼저 하세요." >&2; exit 1; }
# The patches stack (later ones touch the same files as earlier ones), so check the
# series as a whole: apply all of them in order to clean copies of the files they
# touch, taken from rhwp's commit, and compare with vendor/rhwp.
check=$(mktemp -d)
files=$(grep -hE '^(diff --git a/|\+\+\+ b/)' patches/*.patch \
  | sed -E 's|^diff --git a/([^ ]*).*|\1|; s|^\+\+\+ b/([^\t]*).*|\1|' | sort -u)
for f in $files; do
  mkdir -p "$check/$(dirname "$f")"
  git -C vendor/rhwp show "HEAD:$f" > "$check/$f"
done
for p in patches/*.patch; do
  (cd "$check" && git apply "$OLDPWD/$p" 2>/dev/null) \
    || { echo "rhwp 패치를 순서대로 적용할 수 없습니다: $p" >&2; rm -rf "$check"; exit 1; }
done
for f in $files; do
  cmp -s "$check/$f" "vendor/rhwp/$f" \
    || { echo "vendor/rhwp/$f 가 patches/ 와 다릅니다 (README 참고)" >&2; rm -rf "$check"; exit 1; }
done
rm -rf "$check"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture)
rhwp_rev=$(git -C vendor/rhwp rev-parse --short HEAD 2>/dev/null || echo unknown)

cargo build --release -p hwpedit

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
install -Dm755 target/release/hwpedit "$root/usr/bin/hwpedit"
strip "$root/usr/bin/hwpedit"
install -Dm644 packaging/io.github.hwpedit.HwpEdit.desktop "$root/usr/share/applications/io.github.hwpedit.HwpEdit.desktop"
install -Dm644 packaging/io.github.hwpedit.HwpEdit.svg "$root/usr/share/icons/hicolor/scalable/apps/io.github.hwpedit.HwpEdit.svg"
install -Dm644 packaging/hwpedit-mime.xml "$root/usr/share/mime/packages/hwpedit.xml"

doc="$root/usr/share/doc/hwpedit"
install -Dm644 README.md "$doc/README.md"
install -Dm644 docs/01-roundtrip-poc.md "$doc/roundtrip-report.md"
{
  echo "hwpedit $version — MIT License"
  echo
  echo "본 제품은 한컴의 HWP 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다."
  echo
  echo "Document engine: rhwp ($rhwp_rev), https://github.com/edwardkim/rhwp"
  echo "----------------------------------------------------------------------"
  cat vendor/rhwp/LICENSE
  echo
  echo "----------------------------------------------------------------------"
  echo "Bundled Rust crates (name version license):"
  cargo metadata --format-version 1 --filter-platform "$(rustc -vV | sed -n 's/host: //p')" 2>/dev/null \
    | python3 -I -c 'import json,sys; m=json.load(sys.stdin); ws={p["id"] for p in m["packages"] if p["source"] is None and p["name"].startswith("hwpedit")}; [print(p["name"], p["version"], p.get("license") or "see crate") for p in sorted(m["packages"], key=lambda p: p["name"]) if p["id"] not in ws]'
} > "$doc/copyright"
chmod 644 "$doc/copyright"

# Runtime dependencies from the linked shared libraries.
# dpkg-shlibdeps insists on ./debian/control in its working directory.
mkdir -p "$root/DEBIAN" "$root.shlibs/debian"
printf 'Source: hwpedit\n\nPackage: hwpedit\nArchitecture: any\n' > "$root.shlibs/debian/control"
depends=$(cd "$root.shlibs" && dpkg-shlibdeps -O -e"$root/usr/bin/hwpedit" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
rm -rf "$root.shlibs"
[ -n "$depends" ] || { echo "dpkg-shlibdeps 가 의존성을 계산하지 못했습니다" >&2; exit 1; }

cat > "$root/DEBIAN/control" <<CTRL
Package: hwpedit
Version: $version
Architecture: $arch
Maintainer: hwpedit developers <noreply@localhost>
Depends: $depends
Recommends: fonts-nanum
Section: editors
Priority: optional
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Homepage: https://github.com/edwardkim/rhwp
Description: Linux HWP/HWPX document editor (preview)
 GTK4/libadwaita editor for Hangul word processor documents, built on the
 rhwp document engine with a file-integrity layer: unedited documents are
 never rewritten, saves are verified by re-parsing and the previous file is
 backed up under ~/.local/share/hwpedit/backups.
CTRL

mkdir -p dist
out="dist/hwpedit_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$out" >/dev/null
echo "$out"
