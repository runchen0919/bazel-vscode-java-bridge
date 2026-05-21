#!/usr/bin/env bash
#
# Captures thread dumps from ALL Java processes at regular intervals
# for diagnosing stackTrace performance issues during Java debugging.
#
# Usage:
#   ./scripts/capture-jdtls-threadump.sh [interval_seconds] [max_captures]
#
# Defaults: 10-second interval, 120 captures (20 minutes total)
# Output:   logs/threadumps/<processname>-<pid>-<timestamp>.txt

set -euo pipefail

INTERVAL="${1:-10}"
MAX_CAPTURES="${2:-120}"
OUTDIR="$(cd "$(dirname "$0")/.." && pwd)/logs/threadumps"
mkdir -p "$OUTDIR"

echo "=== Java Process Thread Dump Collector ==="
echo "Output directory: $OUTDIR"
echo "Interval: ${INTERVAL}s, Max captures: $MAX_CAPTURES"
echo "Total monitoring time: $(( INTERVAL * MAX_CAPTURES / 60 )) minutes"
echo ""
echo "Will dump ALL Java processes each interval."
echo "Press Ctrl+C to stop early."
echo "---"

for i in $(seq 1 "$MAX_CAPTURES"); do
    TIMESTAMP=$(date '+%Y%m%d_%H%M%S')

    # Find all Java PIDs
    PIDS=$(pgrep -f 'java' 2>/dev/null || true)
    if [ -z "$PIDS" ]; then
        echo "[$TIMESTAMP] Capture $i/$MAX_CAPTURES — no Java processes found, waiting..."
        sleep "$INTERVAL"
        continue
    fi

    PID_COUNT=$(echo "$PIDS" | wc -l | tr -d ' ')
    echo "[$TIMESTAMP] Capture $i/$MAX_CAPTURES — found $PID_COUNT Java process(es)"

    for PID in $PIDS; do
        # Get a short process label from command line
        CMDLINE=$(ps -p "$PID" -o args= 2>/dev/null | head -1 || echo "unknown")

        if echo "$CMDLINE" | grep -q 'equinox.launcher.*jdt'; then
            LABEL="jdtls"
        elif echo "$CMDLINE" | grep -q 'java.debug'; then
            LABEL="debugadapter"
        elif echo "$CMDLINE" | grep -q 'DemoApp\|urbancompass'; then
            LABEL="debuggee"
        else
            # Use the main class or jar name
            LABEL=$(echo "$CMDLINE" | grep -oE '[^ /]+\.jar|[^ /]+\.[A-Z][a-z]+' | tail -1 | sed 's/\.jar//' || echo "java")
            LABEL="${LABEL:-java}"
        fi

        OUTFILE="$OUTDIR/${LABEL}-${PID}-${TIMESTAMP}.txt"
        if jstack "$PID" > "$OUTFILE" 2>&1; then
            THREAD_COUNT=$(grep -c '^"' "$OUTFILE" 2>/dev/null || echo "?")
            echo "  PID=$PID ($LABEL): $THREAD_COUNT threads → $(basename "$OUTFILE")"
        else
            echo "  PID=$PID ($LABEL): jstack failed (may need sudo or process exited)"
            rm -f "$OUTFILE"
        fi
    done

    if [ "$i" -lt "$MAX_CAPTURES" ]; then
        sleep "$INTERVAL"
    fi
done

echo ""
echo "Done. Thread dumps saved to: $OUTDIR"
echo ""
echo "Quick analysis:"
echo "  # List all captured processes:"
echo "  ls $OUTDIR/ | sed 's/-[0-9]*-[0-9_]*.txt//' | sort -u"
echo ""
echo "  # Find threads with debug/stackTrace activity:"
echo "  grep -rl 'java.debug\|StackTrace\|stackTrace\|resolveSource' $OUTDIR/"
echo ""
echo "  # Find BLOCKED threads:"
echo "  grep -rl 'BLOCKED\|waiting to lock' $OUTDIR/"
echo ""
echo "  # Find threads in Bazel classpath code:"
echo "  grep -rl 'com.bazel.jdt' $OUTDIR/"
