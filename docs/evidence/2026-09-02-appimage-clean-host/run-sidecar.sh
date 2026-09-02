#!/bin/bash
# $1=label $2=image $3=mode $4=extra apt $5=docker flags
set -u
LABEL="$1"; IMAGE="$2"; MODE="$3"; EXTRA="${4:-}"; DFLAGS="${5:-}"
APT="apt-get install -y --no-install-recommends xvfb x11-utils imagemagick $EXTRA"
mkdir -p out-sidecar/$LABEL
docker run --rm $DFLAGS \
  -v "$PWD/Beacon-sidecar.AppImage:/opt/beacon/Beacon-sidecar.AppImage:ro" \
  -v "$PWD/probe3.sh:/probe3.sh:ro" -v "$PWD/out-sidecar:/out" \
  --tmpfs /work:exec,size=1g --tmpfs /scratch-home:exec,size=512m \
  "$IMAGE" bash -c "set -e; echo '$APT' > /apt-line.txt; export DEBIAN_FRONTEND=noninteractive; apt-get update -qq >/dev/null 2>&1; $APT >/tmp/apt.log 2>&1 || { echo APT-FAILED; tail -10 /tmp/apt.log; exit 91; }; sh /probe3.sh $LABEL $MODE"
echo "docker exit=$?"
