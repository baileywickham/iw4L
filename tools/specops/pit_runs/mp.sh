#!/bin/bash
cd /Users/baileywickham/workspace/iw4l-specops/.claude/worktrees/agent-adcd4b6c440fa70d9
while pgrep -f "target/play/iw4l" >/dev/null; do sleep 5; done
set -a; . ./.env; set +a
timeout 150 ./target/play/iw4l map mp_boneyard --cmds 'wait world; spawn 0; wait 3s; screenshot mp_check; wait 1s; finish_run' > /dev/null 2>&1
echo "exit=$?"
L=iw4l-artifacts/logs/latest.log
grep -E "gsc: installed|wait spawn|InGame|gsc: refused" $L | cut -c1-160 | head
echo "runtime errors: $(grep -c 'gsc: runtime_error' $L)"
