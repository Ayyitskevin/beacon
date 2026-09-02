#!/bin/sh
# REPAIRED-BUNDLE probe: the shipped AppImage plus the beacond it does not contain.
# This is NOT the shipped artifact. It exists to separate "the bundle is
# incomplete" from "webkit cannot render under Xvfb".
set -u
LABEL="$1"; shift
OUT=/out/$LABEL; mkdir -p "$OUT"
export HOME=/scratch-home; mkdir -p "$HOME"
cd /work || exit 90
echo "=== [$LABEL] env overrides: $* ==="
for kv in "$@"; do export "$kv"; done
cp /opt/beacon/Beacon_0.1.0_amd64.AppImage ./Beacon.AppImage && chmod +x ./Beacon.AppImage
./Beacon.AppImage --appimage-extract >/dev/null 2>&1
cp /opt/beacond/beacond squashfs-root/usr/bin/beacond
chmod +x squashfs-root/usr/bin/beacond
ls -l squashfs-root/usr/bin/

: > "$OUT/stderr.txt"; : > "$OUT/stdout.txt"
xvfb-run -a --server-args="-screen 0 1280x800x24" sh -c "
  ./squashfs-root/AppRun > $OUT/stdout.txt 2> $OUT/stderr.txt &
  APP=\$!
  echo pid=\$APP > $OUT/pid.txt
  i=0
  while [ \$i -lt 24 ]; do
    sleep 1; i=\$((i+1))
    if ! kill -0 \$APP 2>/dev/null; then echo \"DIED after \${i}s\" >> $OUT/pid.txt; break; fi
    if [ \$i -eq 6 ] || [ \$i -eq 12 ] || [ \$i -eq 20 ]; then
      xwininfo -root -tree > $OUT/xwininfo-\${i}s.txt 2>&1
      import -window root $OUT/root-\${i}s.png 2>> $OUT/capture.err
      WID=\$(xwininfo -root -tree 2>/dev/null | grep -i '\"Beacon\"' | head -1 | awk '{print \$1}')
      echo \"beacon window at \${i}s: \$WID\" >> $OUT/pid.txt
      [ -n \"\$WID\" ] && import -window \$WID $OUT/win-\${i}s.png 2>> $OUT/capture.err
    fi
  done
  kill -0 \$APP 2>/dev/null && echo ALIVE_AT_24S >> $OUT/pid.txt || echo NOT_ALIVE_AT_24S >> $OUT/pid.txt
  kill \$APP 2>/dev/null
"
echo "=== [$LABEL] liveness ==="; cat "$OUT/pid.txt"
echo "=== [$LABEL] stderr ==="; cat "$OUT/stderr.txt"
echo "=== [$LABEL] beacond log ==="; cat "$HOME"/.local/share/*/runtime/beacond.log 2>/dev/null | head -20
echo "=== [$LABEL] captures ==="
for f in "$OUT"/*.png; do
  [ -e "$f" ] || continue
  echo "$(basename "$f") mean=$(convert "$f" -format '%[fx:mean]' info: 2>/dev/null) colors=$(convert "$f" -format '%k' info: 2>/dev/null) bytes=$(stat -c%s "$f")"
done
echo "=== [$LABEL] window tree ==="; cat "$OUT/xwininfo-20s.txt" 2>/dev/null | head -15
