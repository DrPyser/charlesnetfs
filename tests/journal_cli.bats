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

teardown_file() {
    stop_journald_gateway "$JOURNALD_CONTAINER"
}

@test "logger can write to container journal via dev-log socket" {
    run docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "test message from bats"

    run docker exec "$JOURNALD_CONTAINER" journalctl -b -n 1 --no-pager
    assert_output --partial "test message from bats"
}

@test "journal cli can query journald-gateway API" {
    run docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "test message from bats"

    run $JOURNAL_CLI localhost $GATEWAY_PORT
    assert_output --partial "test message from bats"
}

@test "journal cli query limit" {
    for i in {1..20}; do
        echo "test message #$i"
    done | run docker exec -i "$JOURNALD_CONTAINER" xargs -L1 logger -u /run/systemd/journal/dev-log

    run --separate-stderr $JOURNAL_CLI --limit 10 localhost $GATEWAY_PORT
    [[ ${#lines[@]} = 10 ]]
}

@test "journal cli follow blocks to wait for log lines" {
    OUTPUT=$BATS_TEST_TMPDIR/output
    $JOURNAL_CLI --follow localhost $GATEWAY_PORT >"$OUTPUT" &
    FOLLOWPID=$!
    trap "kill $FOLLOWPID" EXIT
    sleep 0.5
    # check that the process is still running
    assert kill -0 "$FOLLOWPID"

    # push some log entries
    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "test message from bats"
    run wait_for $OUTPUT 'test message from bats' >&2
    assert_success
}
