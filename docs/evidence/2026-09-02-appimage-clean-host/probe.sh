#!/bin/sh
# Clean-host first-launch probe. Runs INSIDE a disposable container.
# $1 = label, $2 = "fuse" (direct run) | "extract" (--appimage-extract-and-run)
set -u
LABEL="$1"; MODE="$2"
OUT=/out/$LABEL
mkdir -p "$OUT"
export HOME=/scratch-home
mkdir -p "$HOME"
cd /work || exit 90

echo "=== [$LABEL] image identity ==="
cat /etc/os-release | head -3
echo "glibc: $(ldd --version 2>&1 | head -1)"

echo "=== [$LABEL] apt line ==="
cat /apt-line.txt

cp /opt/beacon/Beacon_0.1.0_amd64.AppImage ./Beacon.AppImage
chmod +x ./Beacon.AppImage

echo "=== [$LABEL] extract for inspection (never launches anything) ==="
./Beacon.AppImage --appimage-extract > "$OUT/extract.log" 2>&1
echo "extract exit=$?"
BIN=$(find squashfs-root -type f -name 'beacon-desktop' | head -1)
echo "binary: $BIN"
file "$BIN" > "$OUT/file.txt" 2>&1; cat "$OUT/file.txt"

echo "=== [$LABEL] ldd against the CONTAINER's libraries ==="
ldd "$BIN" > "$OUT/ldd.txt" 2>&1
grep -c . "$OUT/ldd.txt"
echo "--- missing ---"
grep "not found" "$OUT/ldd.txt" | sort -u | tee "$OUT/missing-libs.txt"
echo "missing count: $(grep -c 'not found' "$OUT/ldd.txt")"

echo "=== [$LABEL] launch mode=$MODE ==="
if [ "$MODE" = "fuse" ]; then
  CMD="./Beacon.AppImage"
else
  CMD="./Beacon.AppImage --appimage-extract-and-run"
fi
echo "command: xvfb-run -a --server-args=\"-screen 0 1280x800x24\" $CMD"

: > "$OUT/stderr.txt"; : > "$OUT/stdout.txt"
xvfb-run -a --server-args="-screen 0 1280x800x24" sh -c "
  $CMD > $OUT/stdout.txt 2> $OUT/stderr.txt &
  APP=\$!
  echo \"pid=\$APP\" > $OUT/pid.txt
  i=0
  while [ \$i -lt 24 ]; do
    sleep 1
    i=\$((i+1))
    if ! kill -0 \$APP 2>/dev/null; then echo \"DIED after \${i}s\" >> $OUT/pid.txt; break; fi
    if [ \$i -eq 6 ] || [ \$i -eq 12 ] || [ \$i -eq 20 ]; then
      xwininfo -root -tree > $OUT/xwininfo-\${i}s.txt 2>&1
      import -window root $OUT/root-\${i}s.png 2>> $OUT/capture.err
      WID=\$(xwininfo -root -tree 2>/dev/null | grep -i beacon | head -1 | awk '{print \$1}')
      echo \"window id at \${i}s: \$WID\" >> $OUT/pid.txt
      if [ -n \"\$WID\" ]; then import -window \$WID $OUT/win-\${i}s.png 2>> $OUT/capture.err; fi
    fi
  done
  if kill -0 \$APP 2>/dev/null; then echo ALIVE_AT_24S >> $OUT/pid.txt; else echo NOT_ALIVE_AT_24S >> $OUT/pid.txt; fi
  kill \$APP 2>/dev/null
  wait \$APP 2>/dev/null
  echo \"app exit=\$?\" >> $OUT/pid.txt
"
echo "xvfb-run exit=$?"

echo "=== [$LABEL] liveness ==="
cat "$OUT/pid.txt"
echo "=== [$LABEL] stderr (verbatim) ==="
cat "$OUT/stderr.txt"
echo "=== [$LABEL] stdout (verbatim) ==="
cat "$OUT/stdout.txt"
echo "=== [$LABEL] captures ==="
for f in "$OUT"/*.png; do
  [ -e "$f" ] || continue
  M=$(convert "$f" -format "%[fx:mean]" info: 2>/dev/null)
  C=$(convert "$f" -format "%k" info: 2>/dev/null)
  echo "$(basename "$f") mean=$M colors=$C bytes=$(stat -c%s "$f")"
done
echo "=== [$LABEL] window tree at 20s ==="
cat "$OUT/xwininfo-20s.txt" 2>/dev/null | head -20
