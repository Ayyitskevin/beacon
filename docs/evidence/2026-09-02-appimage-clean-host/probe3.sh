#!/bin/sh
# Clean-host first launch of the SIDECAR AppImage. Runs INSIDE a disposable container.
# $1=label $2=fuse|extract
set -u
LABEL="$1"; MODE="$2"
OUT=/out/$LABEL; mkdir -p "$OUT"
export HOME=/scratch-home; mkdir -p "$HOME"
cd /work || exit 90
echo "=== [$LABEL] $(grep PRETTY /etc/os-release) | $(ldd --version 2>&1 | head -1) ==="
cat /apt-line.txt
cp /opt/beacon/Beacon-sidecar.AppImage ./Beacon.AppImage && chmod +x ./Beacon.AppImage

echo "=== [$LABEL] bundle contains a supervisor? ==="
./Beacon.AppImage --appimage-extract >/dev/null 2>&1
ls -l squashfs-root/usr/bin/
echo "--- ldd missing (shell) ---"
ldd squashfs-root/usr/bin/beacon-desktop 2>&1 | grep "not found" | sed 's/^\s*//' | sort -u | tee "$OUT/missing-shell.txt"
echo "--- ldd missing (supervisor) ---"
ldd squashfs-root/usr/bin/beacond 2>&1 | grep "not found" | sed 's/^\s*//' | sort -u | tee "$OUT/missing-beacond.txt"

if [ "$MODE" = "fuse" ]; then CMD="./Beacon.AppImage"; else CMD="./Beacon.AppImage --appimage-extract-and-run"; fi
echo "=== [$LABEL] launch: xvfb-run -a --server-args=\"-screen 0 1280x800x24\" $CMD ==="
: > "$OUT/stderr.txt"; : > "$OUT/stdout.txt"
xvfb-run -a --server-args="-screen 0 1280x800x24" sh -c "
  $CMD > $OUT/stdout.txt 2> $OUT/stderr.txt &
  APP=\$!
  echo pid=\$APP > $OUT/pid.txt
  i=0
  while [ \$i -lt 24 ]; do
    sleep 1; i=\$((i+1))
    kill -0 \$APP 2>/dev/null || { echo \"DIED after \${i}s\" >> $OUT/pid.txt; break; }
    if [ \$i -eq 8 ] || [ \$i -eq 20 ]; then
      xwininfo -root -tree > $OUT/xwininfo-\${i}s.txt 2>&1
      import -window root $OUT/root-\${i}s.png 2>> $OUT/capture.err
      WID=\$(xwininfo -root -tree 2>/dev/null | grep '\"Beacon\"' | head -1 | awk '{print \$1}')
      echo \"beacon window at \${i}s: \$WID\" >> $OUT/pid.txt
      [ -n \"\$WID\" ] && import -window \$WID $OUT/win-\${i}s.png 2>> $OUT/capture.err
      echo \"--- supervisor processes at \${i}s ---\" >> $OUT/procs.txt
      ps -eo pid,ppid,comm,args 2>/dev/null | grep -v grep | grep beacond >> $OUT/procs.txt
    fi
  done
  kill -0 \$APP 2>/dev/null && echo ALIVE_AT_24S >> $OUT/pid.txt || echo NOT_ALIVE_AT_24S >> $OUT/pid.txt
  kill \$APP 2>/dev/null
"
echo "=== [$LABEL] liveness ==="; cat "$OUT/pid.txt"
echo "=== [$LABEL] supervisor process ==="; cat "$OUT/procs.txt" 2>/dev/null || echo "(none seen)"
echo "=== [$LABEL] scratch HOME after the run ==="
find "$HOME" -type f 2>/dev/null | head -20 | tee "$OUT/home-tree.txt"
echo "--- store file ---"; ls -l "$HOME"/.local/share/*/beacon.sqlite3 2>/dev/null || echo "(no store)"
echo "--- beacond log ---"; cat "$HOME"/.local/share/*/runtime/beacond.log 2>/dev/null | head -10
echo "=== [$LABEL] stderr (verbatim) ==="; cat "$OUT/stderr.txt"
echo "=== [$LABEL] stdout (verbatim) ==="; cat "$OUT/stdout.txt"
echo "=== [$LABEL] captures ==="
for f in "$OUT"/*.png; do [ -e "$f" ] || continue
  echo "$(basename $f) mean=$(convert $f -format '%[fx:mean]' info: 2>/dev/null) colors=$(convert $f -format '%k' info: 2>/dev/null) bytes=$(stat -c%s $f)"; done
echo "=== [$LABEL] window tree ==="; head -12 "$OUT/xwininfo-20s.txt" 2>/dev/null
