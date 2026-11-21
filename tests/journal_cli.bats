#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

load 'test_helper/helpers'
load 'test_helper/bats-assert/load.bash'
load 'test_helper/bats-support/load.bash'

JOURNALD_CONTAINER="charlesnetfs-journald-cli"
JOURNAL_CLI="cargo run --bin journal --"

setup_file() {
    start_journald_gateway "$JOURNALD_CONTAINER"
}

teardown() {
    # Kill leftover follow process (timeout forwards to cargo + journal)
    if [ -f "$BATS_TEST_TMPDIR/follow.pid" ]; then
        kill -TERM $(cat "$BATS_TEST_TMPDIR/follow.pid") 2>/dev/null || true
        rm -f "$BATS_TEST_TMPDIR/follow.pid"
    fi
}

teardown_file() {
    stop_journald_gateway "$JOURNALD_CONTAINER"
}

log_to_journal() {
    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log $1
}

@test "logger can write to container journal via dev-log socket" {
    log_to_journal "test message from bats"

    run docker exec "$JOURNALD_CONTAINER" journalctl -b -n 1 --no-pager
    assert_output --partial "test message from bats"
}

@test "journal cli can query journald-gateway API" {
    log_to_journal "test message from bats"

    run $JOURNAL_CLI localhost $GATEWAY_PORT
    assert_output --partial "test message from bats"
}

@test "journal cli query limit" {
    for i in {1..20}; do
        log_to_journal "test message #$i"
    done

    run --separate-stderr $JOURNAL_CLI --limit 10 localhost $GATEWAY_PORT
    [[ ${#lines[@]} = 10 ]]
}

@test "journal cli follow blocks to wait for log lines" {
    OUTPUT=$BATS_TEST_TMPDIR/output
    # timeout wraps cargo+journal — killing timeout PID kills the whole tree
    timeout 30 $JOURNAL_CLI --follow localhost $GATEWAY_PORT >"$OUTPUT" 2>/dev/null &
    FOLLOWPID=$!
    echo $FOLLOWPID > "$BATS_TEST_TMPDIR/follow.pid"
    sleep 0.5
    # check that the process is still running
    assert kill -0 "$FOLLOWPID"

    # push some log entries
    log_to_journal "test message from bats"
    run timeout 5 grep -m 1 'test message from bats' <(tail -f "$OUTPUT" 2>/dev/null)
    assert_success
}
