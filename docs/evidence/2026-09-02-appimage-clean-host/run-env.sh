#!/bin/bash
# $1=label $2=image $3=mode(fuse|extract) $4=extra apt packages (may be empty)
set -u
LABEL="$1"; IMAGE="$2"; MODE="$3"; EXTRA="${4:-}"
HARNESS="xvfb x11-utils imagemagick"
APT="apt-get install -y --no-install-recommends $HARNESS $EXTRA"
mkdir -p out/$LABEL
docker run --rm \
  -v "$PWD/Beacon_0.1.0_amd64.AppImage:/opt/beacon/Beacon_0.1.0_amd64.AppImage:ro" \
  -v "$PWD/probe.sh:/probe.sh:ro" \
  -v "$PWD/out:/out" \
  --tmpfs /work:exec,size=1g --tmpfs /scratch-home:size=256m \
  "$IMAGE" \
  bash -c "set -e; echo '$APT' > /apt-line.txt; export DEBIAN_FRONTEND=noninteractive; apt-get update -qq >/dev/null 2>&1; $APT >/tmp/apt.log 2>&1 || { echo APT-FAILED; tail -20 /tmp/apt.log; exit 91; }; sh /probe.sh $LABEL $MODE"
echo "docker exit=$?"
