#!/bin/bash
# Host lobby screenshot for a Spec Ops zone on the Air (local master). usage: air_lobby.sh <zone>
# Needs a prior air_duo.sh run (synced tree, built binaries, master certs).
set -u
D=$(cd "$(dirname "$0")" && pwd); WT=$(cd "$D/../.." && pwd)
ZONE=${1:-iw4:so_killspree_invasion}
SSH="ssh -i $HOME/.ssh/macnode -o BatchMode=yes baileywickham@192.168.0.50"
R="runs/$(basename "$WT")"
$SSH "cd ~/$R && export PATH=\$HOME/.cargo/bin:/opt/homebrew/bin:\$PATH && printf 'IW4L_GAMES=%s/Games\nIW4L_MASTER_ADDR=127.0.0.1:4433\nIW4L_MASTER_SERVER_NAME=iw4l-dev\nIW4L_MASTER_CA_CERT=%s\n' \$HOME \$HOME/$R-ca/iw4l-ca.pem > .env.lobby && set -a && . ./.env.lobby && set +a && \
  L=\$HOME/runs/.game.lock; until mkdir \$L 2>/dev/null; do sleep 5; done; \
  trap 'rmdir \$L 2>/dev/null; pkill -f \"iw4l-master serve --bind 127.0.0.1:4433\"' EXIT; while pgrep -x iw4l >/dev/null; do sleep 5; done; \
  ./target/play/iw4l-master serve --bind 127.0.0.1:4433 --cert \$HOME/$R-ca/server-cert.pem --key \$HOME/$R-ca/server-key.pem --updates /tmp/iw4l-master-updates > /tmp/iw4l-master-lobby.log 2>&1 & \
  sleep 2; rm -rf iw4l-artifacts/screenshots; \
  timeout 120 ./target/play/iw4l menu --cmds 'set ui_mapname $ZONE; set ui_gametype so; ui_create_lobby; wait 6s; screenshot lobby; wait 1s; finish_run' > /tmp/iw4l-lobby.out 2>&1; echo exit=\$?"
mkdir -p "$D/runs/lobby"
rsync -a -e "ssh -i $HOME/.ssh/macnode" "baileywickham@192.168.0.50:$R/iw4l-artifacts/screenshots/" "$D/runs/lobby/"
ls "$D/runs/lobby"
