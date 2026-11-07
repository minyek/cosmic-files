#!/usr/bin/bash
# lines-at.sh <listed-file> <end-rev>: AC 5 per-line check and added-lines identity over e03001c..<end-rev>.
H=/mnt/logs/temp/history-rebuild; R=/mnt/work/projects/cosmic-de/cosmic-files
"$H/bin/check-lines.py" "$R" e03001c "$2" "$1" --report "$H/commits/lines-report.tsv"
n=$(grep -c . "$H/commits/lines-report.tsv")
"$H/bin/added-identity.py" "$R" e03001c "$2" "$n"
