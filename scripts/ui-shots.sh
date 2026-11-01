#!/bin/bash
# Save the window with each ribbon tab in front, at 2x, for reviewing the UI
# without clicking: .tmp/ui/shots/<scheme>[-clean]/<tab>.png
#
#   scripts/ui-shots.sh dark|light [clean] [DOC]
#
# clean: ignore ~/.config (a GTK theme there), as a user without one sees it.
# Needs Xvfb (display :9) and a release build (cargo build --release -p hwpedit).
set -u
cd "$(dirname "$0")/.."
scheme=${1:-dark}
tag=$scheme${2:+-$2}
doc=${3:-vendor/rhwp/saved/blank2010.hwp}
out=$PWD/.tmp/ui/shots/$tag
rm -rf "$out"; mkdir -p "$out"
cfg=()
[ "${2:-}" = clean ] && { mkdir -p .tmp/ui/cfg; cfg=(XDG_CONFIG_HOME=$PWD/.tmp/ui/cfg); }
pidof Xvfb >/dev/null || { (Xvfb :9 -screen 0 1600x1000x24 > /dev/null 2>&1 &); sleep 1.5; }
env "${cfg[@]}" ADW_DEBUG_COLOR_SCHEME=prefer-$scheme HWPEDIT_UI_SHOTS="$out" GDK_BACKEND=x11 DISPLAY=:9 \
    ./target/release/hwpedit "$PWD/$doc" > "$out.log" 2>&1 &
pid=$!
for _ in $(seq 1 60); do [ -e "$out/done" ] && break; sleep 0.5; done
kill $pid 2>/dev/null
ls "$out"
