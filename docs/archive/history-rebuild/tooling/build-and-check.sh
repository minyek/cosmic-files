#!/usr/bin/bash
# build-and-check.sh <order-file>: rebuild the series from the order, then run check-commit.py over every commit.
set -e
/mnt/logs/temp/history-rebuild/bin/build-series.py "$1"
mv /mnt/logs/temp/history-rebuild/commits/check-summary.txt /mnt/logs/temp/history-rebuild/commits/check-summary-prev.txt 2>/dev/null || true
/mnt/logs/temp/history-rebuild/bin/check-commit.py history-rebuild-series
