#!/bin/bash
# usage: run.sh <logname> <cmds> [timeout]
cd /Users/baileywickham/workspace/iw4l-specops/.claude/worktrees/agent-adcd4b6c440fa70d9
while pgrep -f "target/play/iw4l" >/dev/null; do sleep 5; done
set -a; . ./.env; set +a
export IW4L_GSC_STUB_NATIVES=1
LOG=/private/tmp/claude-501/-Users-baileywickham-workspace/039ca697-ddd6-448c-97c7-256520391cd0/scratchpad/so/$1.log
timeout ${3:-200} ./target/play/iw4l map so_killspree_trainer --cmds "$2" > $LOG 2>&1
echo "exit=$?"
echo "runtime errors: $(grep -c 'gsc: script runtime error\|gsc: runtime_error' $LOG)"
grep -o 'gsc: stubbed n=[0-9]*' $LOG
