#!/bin/sh

task run
exit_code=$?

if [ "$exit_code" -eq 0 ]; then
  # A clean app exit means the user quit the dev session.
  kill -TERM "$MAZIT_DEV_WATCHER_PID"
fi

exit "$exit_code"
