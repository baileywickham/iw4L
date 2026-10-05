#!/bin/bash
# usage: runf.sh <logname> <cmdsfile> [timeout]
D=/private/tmp/claude-501/-Users-baileywickham-workspace/039ca697-ddd6-448c-97c7-256520391cd0/scratchpad/so
exec "$D/run.sh" "$1" "$(cat "$2")" "${3:-240}"
