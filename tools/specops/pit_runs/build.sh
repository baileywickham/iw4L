#!/bin/bash
cd /Users/baileywickham/workspace/iw4l-specops/.claude/worktrees/agent-adcd4b6c440fa70d9
export PATH=$HOME/.cargo/bin:$PATH
cargo build --profile play -p launcher 2>&1 | grep -E "^(error|warning: unused)" -A 14 | head -${1:-60}
echo "build exit=${PIPESTATUS[0]}"
