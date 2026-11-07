#!/usr/bin/bash
rm -f /mnt/logs/temp/history-rebuild/commits/selfcontained.tsv
exec /mnt/logs/temp/history-rebuild/bin/check-selfcontained.py e03001c history-rebuild-series
