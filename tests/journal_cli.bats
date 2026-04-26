#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

load 'test_helper/helpers'
load 'test_helper/bats-assert/load.bash'
load 'test_helper/bats-support/load.bash'

JOURNALD_IMAGE="charlesnetfs-journald-gateway"
JOURNALD_CONTAINER="charlesnetfs-journald-test"
JOURNAL_CLI="cargo run --bin journal --"

start_journald_gateway() {
    if ! command -v docker >/dev/null 2>&1; then
        return 1
    fi

    # Remove any existing container
    docker rm -f "$JOURNALD_CONTAINER" >/dev/null 2>&1 || true
    docker build -f tests/data/journald-gateway/Dockerfile -t "$JOURNALD_IMAGE" .

    # Start journald + journal-gateway container
    docker run -d --name "$JOURNALD_CONTAINER" -p 19531 \
        "$JOURNALD_IMAGE"
    export gateway_port=$(docker port "$JOURNALD_CONTAINER" 19531/tcp | cut -d':' -f2)

    # Wait for port to be available
    for i in {1..10}; do
        if curl -s http://localhost:$gateway_port/entries >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

stop_journald_gateway() {
    docker stop "$JOURNALD_CONTAINER" >/dev/null 2>&1 || true
    docker rm -f "$JOURNALD_CONTAINER" >/dev/null 2>&1 || true
}

setup_file() {
    start_journald_gateway
}

@test "logger can write to container journal via dev-log socket" {
    # Test that logger -u can write to the container's journal
    run docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "test message from bats"

    # Check that the message appears in the journal
    run docker exec "$JOURNALD_CONTAINER" journalctl -b -n 1 --no-pager
    assert_output --partial "test message from bats"
}

@test "journal cli can query journald-gateway API" {
    run docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "test message from bats"

    # Check that the message appears in the journal
    run $JOURNAL_CLI localhost $gateway_port
    assert_output --partial "test message from bats"
}

@test "journal cli query limit" {
    for i in {1..20}; do
        echo "test message #$i"
    done | run docker exec -i "$JOURNALD_CONTAINER" xargs -L1 logger -u /run/systemd/journal/dev-log

    # Check that the message appears in the journal
    run --separate-stderr $JOURNAL_CLI --limit 10 localhost $gateway_port
    [[ ${#lines[@]} = 10 ]]
}

teardown_file() {
    stop_journald_gateway
}
