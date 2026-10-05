#!/bin/bash
# Fresh duo + wait for level entry + key lines.
cd /Users/baileywickham/workspace/iw4l-specops/.claude/worktrees/agent-a08ca9e9969e2d154
echo quit >> context/duo.cmds; sleep 6; pkill -f "tail -f context/duo.cmds"; sleep 2
(./context/duo.sh > context/duo.out 2>&1 &)
sleep 5
./context/watch.sh ${1:-15} 30 | grep "spec ops: client\|level entry"
./context/q.sh grep "gsc print\|TMPDIAG\|fault" host 12 300
