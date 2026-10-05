#!/bin/bash
# summarize latest log
L=/Users/baileywickham/workspace/iw4l-specops/.claude/worktrees/agent-adcd4b6c440fa70d9/iw4l-artifacts/logs/latest.log
echo "errors: $(grep -c 'gsc: runtime_error' $L)"
grep -o 'gsc: stubbed n=[0-9]*' $L
grep -o 'gsc: runtime_error.*' $L | sed -E 's/pid=[0-9]+ ns=[0-9]+ //' | sed -E 's/at=([^:]+):[0-9]+:[0-9]+/at=\1/' | sed -E "s/'[^']*'/'X'/g" | sort | uniq -c | sort -rn | cut -c1-220 | head -${1:-60}
