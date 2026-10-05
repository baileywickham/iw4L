#!/bin/bash
# Two local clients side by side through the local master. usage: duo.sh <zone> [mode]
# Console lines: append "host ...", "client ...", "both ...", "quit" to context/duo.cmds.
W=/Users/baileywickham/workspace/iw4l-specops/.claude/worktrees/agent-a08ca9e9969e2d154
cd $W
export PATH=$HOME/.cargo/bin:$PATH
while pgrep -x iw4l >/dev/null; do sleep 5; done
set -a; . ./.env; set +a
export IW4L_MASTER_ADDR=127.0.0.1:4433 IW4L_MASTER_SERVER_NAME=iw4l-dev IW4L_MASTER_CA_CERT=$W/context/master-ca/iw4l-ca.pem
export IW4L_GSC_STUB_NATIVES=1 IW4L_SCRIPT_OVERRIDE=${IW4L_SCRIPT_OVERRIDE-$W/context/override}
export ZONE=${1:-iw4:so_killspree_trainer} MODE=${2:-so} PROFILE=play
export DUO_RESOLUTION=${DUO_RESOLUTION:-740x420} DUO_HOST_POS=0,80 DUO_CLIENT_POS=1530,80
export HOST_CMDS="${HOST_CMDS:-spawn 0}" CLIENT_CMDS="${CLIENT_CMDS:-spawn 0}"
: > context/duo.cmds
tail -f context/duo.cmds | cargo run --quiet -p xtask -- duo
