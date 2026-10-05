#!/bin/bash
# Co-op duo (host + client + local master) on the spare Air.
# usage: air_duo.sh <zone> <name> <seconds>   with HOST_CMDS / CLIENT_CMDS / DUO_ENV in the environment.
# Results: context/coop/runs/<name>/{host,client}/ (logs, screenshots).
set -u
D=$(cd "$(dirname "$0")" && pwd); WT=$(cd "$D/../.." && pwd)
ZONE=$1; NAME=$2; SECS=${3:-180}
SSH="ssh -i $HOME/.ssh/macnode -o BatchMode=yes baileywickham@192.168.0.50"
R="runs/$(basename "$WT")"
rsync -a --delete -e "ssh -i $HOME/.ssh/macnode" --exclude target --exclude iw4l-artifacts --exclude .claude --exclude context "$WT/" "baileywickham@192.168.0.50:$R/"
rsync -a -e "ssh -i $HOME/.ssh/macnode" "$D/master-ca/" "baileywickham@192.168.0.50:$R-ca/"
q() { printf '%q' "$1"; }
$SSH "cd ~/$R && export PATH=\$HOME/.cargo/bin:/opt/homebrew/bin:\$PATH && \
  (cargo build --profile play -p launcher 2>&1 | grep -E '^error|Finished' -A8 | head -40) && (cargo build --profile play -p iw4l-master 2>&1 | grep -E '^error|Finished' -A8 | head -40) && \
  (cargo build -q -p xtask 2>&1 | grep -E '^error' -A8 | head -20); \
  printf 'IW4L_GAMES=%s/Games\nIW4L_MASTER_ADDR=127.0.0.1:4433\nIW4L_MASTER_SERVER_NAME=iw4l-dev\nIW4L_MASTER_CA_CERT=%s\n' \$HOME \$HOME/$R-ca/iw4l-ca.pem > .env && set -a && . ./.env && set +a && \
  L=\$HOME/runs/.game.lock; until mkdir \$L 2>/dev/null; do [ -n \"\$(find \$L -maxdepth 0 -mmin +45 2>/dev/null)\" ] && rmdir \$L; sleep 5; done; \
  trap 'rmdir \$L 2>/dev/null; pkill -f \"iw4l-master serve --bind 127.0.0.1:4433\"' EXIT; while pgrep -x iw4l >/dev/null; do sleep 5; done; \
  mkdir -p /tmp/iw4l-master-updates; \
  ./target/play/iw4l-master serve --bind 127.0.0.1:4433 --cert \$HOME/$R-ca/server-cert.pem --key \$HOME/$R-ca/server-key.pem --updates /tmp/iw4l-master-updates > /tmp/iw4l-master-$NAME.log 2>&1 & \
  sleep 2; \
  export ZONE=$(q "$ZONE") MODE=so PROFILE=play IW4L_GSC_STUB_NATIVES=1 IW4L_GSC_TRACE_LEVEL=1 ${DUO_ENV:-} \
    DUO_RESOLUTION=700x420 DUO_HOST_POS=0,60 DUO_CLIENT_POS=720,60 \
    HOST_CMDS=$(q "$HOST_CMDS") CLIENT_CMDS=$(q "$CLIENT_CMDS"); \
  (sleep $SECS; echo quit) | timeout $((SECS + 120)) ./target/debug/xtask duo > /tmp/iw4l-duo-$NAME.out 2>&1; \
  echo duo exit=\$?; ls -td iw4l-artifacts/duo/* | head -1 > /tmp/iw4l-duo-$NAME.dir"
DIR=$($SSH "cat /tmp/iw4l-duo-$NAME.dir")
mkdir -p "$D/runs/$NAME"
rsync -a -e "ssh -i $HOME/.ssh/macnode" --exclude cache "baileywickham@192.168.0.50:$R/$DIR/" "$D/runs/$NAME/"
$SSH "cat /tmp/iw4l-duo-$NAME.out" | tail -5
echo "duo: $D/runs/$NAME"
find "$D/runs/$NAME" -name '*.png' | sort
