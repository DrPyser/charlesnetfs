#!/usr/bin/env bats

load 'test_helper/helpers'

JOURNALD_CONTAINER="charlesnetfs-journald-bats"

setup_file() {
    _setup_file

    start_journald_gateway "$JOURNALD_CONTAINER"

    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "bats message one"
    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "bats message two"

    export JOURNAL_CONFIG="$BATS_FILE_TMPDIR/config.yaml"
    cat > "$JOURNAL_CONFIG" <<EOF
global_timeout_ms: 5000
journal_limit: 10
targets:
  gateway:
    addr: 127.0.0.1:${GATEWAY_PORT}
    journal_port: ${GATEWAY_PORT}
EOF
}

setup() {
    common_setup
    start_server "$JOURNAL_CONFIG"
    wait_for_socket
}

teardown() {
    common_teardown
}

teardown_file() {
    stop_journald_gateway "$JOURNALD_CONTAINER"
}

# Listing and stat tests

@test "target directory lists journal file via 9p" {
    result=$(9 9p ls charlesnetfs/gateway/)

    [[ "$result" == *"journal"* ]]
    [[ "$result" == *"ping"* ]]
    [[ "$result" == *"config"* ]]
}

@test "journal file is visible via 9p stat" {
    result=$(9 9p stat charlesnetfs/gateway/journal)

    [[ "$result" == *"journal"* ]]
}

@test "journal file is read-only via 9p stat" {
    result=$(9 9p stat charlesnetfs/gateway/journal)

    [[ "$result" == *'m 0444'* ]]
}

@test "fuse mount shows journal file in target directory" {
    mount_fuse

    [ -f "$MOUNT_DIR/gateway/journal" ]
}

@test "fuse mount journal file is read-only" {
    mount_fuse

    perms=$(stat -c %a "$MOUNT_DIR/gateway/journal")
    [ "$perms" = "444" ]
}

@test "fuse mount journal file is not a directory" {
    mount_fuse

    [ -f "$MOUNT_DIR/gateway/journal" ]
    [ ! -d "$MOUNT_DIR/gateway/journal" ]
}

# Happy path read tests (gateway available)

@test "9p read journal returns content" {
    result=$(9 9p read charlesnetfs/gateway/journal)

    [[ "$result" == *"bats message one"* ]]
    [[ "$result" == *"bats message two"* ]]
}

@test "9p read journal contains timestamp format" {
    result=$(9 9p read charlesnetfs/gateway/journal)

    [[ "$result" == *"["* ]]
}

@test "fuse read journal returns content" {
    mount_fuse

    result=$(cat "$MOUNT_DIR/gateway/journal")

    [[ "$result" == *"bats message one"* ]]
    [[ "$result" == *"bats message two"* ]]
}

@test "fuse read journal is readable" {
    mount_fuse

    [ -r "$MOUNT_DIR/gateway/journal" ]
}

@test "9p read journal shows new entries after write" {
    result=$(9 9p read charlesnetfs/gateway/journal)
    [[ "$result" == *"bats message one"* ]]

    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "bats message three"

    result=$(9 9p read charlesnetfs/gateway/journal)
    [[ "$result" == *"bats message three"* ]]
}

@test "9p read journal blocks until new entry arrives" {
    # Read to consume existing entries
    9 9p read charlesnetfs/gateway/journal >/dev/null

    # Start a read that should block at EOF
    output_file="$BATS_TMPDIR/journal_read_output"
    9 9p read charlesnetfs/gateway/journal > "$output_file" &
    read_pid=$!

    # Give the read time to reach EOF and block
    sleep 0.5

    # File should be empty (still blocking)
    [ ! -s "$output_file" ]
    kill -0 "$read_pid"
    [ "$!" -eq 0 ]

    # Write new entry to unblock the read
    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "unblocking message"

    # Wait for read to complete
    timeout 5 tail --pid=$read_pid -f /dev/null 2>/dev/null || true
    wait $read_pid 2>/dev/null || true

    # Should contain the new entry
    result=$(cat "$output_file")
    [[ "$result" == *"unblocking message"* ]]
}

@test "fuse read journal blocks until new entry arrives" {
    mount_fuse

    # Read to consume existing entries
    cat "$MOUNT_DIR/gateway/journal" >/dev/null

    # Start a read that should block at EOF
    output_file="$BATS_TMPDIR/journal_fuse_read_output"
    rm -f "$output_file"
    cat "$MOUNT_DIR/gateway/journal" > "$output_file" &
    read_pid=$!

    # Give the read time to reach EOF and block
    sleep 0.5

    # File should be empty (still blocking)
    [ ! -s "$output_file" ]

    # Write new entry to unblock the read
    docker exec "$JOURNALD_CONTAINER" logger -u /run/systemd/journal/dev-log "unblocking fuse message"

    # Wait for read to complete (with timeout)
    timeout 5 tail --pid=$read_pid -f /dev/null 2>/dev/null || true
    wait $read_pid 2>/dev/null || true

    # Should contain the new entry
    result=$(cat "$output_file")
    [[ "$result" == *"unblocking fuse message"* ]]
}

# Error path tests (gateway unavailable)

@test "9p read journal fails when gateway is down" {
    docker stop "$JOURNALD_CONTAINER" >/dev/null 2>&1

    run 9 9p read charlesnetfs/gateway/journal
    [ "$status" -ne 0 ]

    docker start "$JOURNALD_CONTAINER" >/dev/null 2>&1
}

@test "fuse read journal fails when gateway is down" {
    mount_fuse
    docker stop "$JOURNALD_CONTAINER" >/dev/null 2>&1

    run cat "$MOUNT_DIR/gateway/journal"
    [ "$status" -ne 0 ]

    docker start "$JOURNALD_CONTAINER" >/dev/null 2>&1
}

# Hot-reload test

@test "hot-reload config includes journal files for new targets" {
    result=$(9 9p ls charlesnetfs/)
    [[ "$result" == *"gateway"* ]]

    new_config="global_timeout_ms: 5000
journal_limit: 25
targets:
  newhost:
    addr: 10.0.0.99:22
    journal_port: 19531
"
    echo "$new_config" | 9 9p write charlesnetfs/config

    result=$(9 9p ls charlesnetfs/newhost/)
    [[ "$result" == *"journal"* ]]
    [[ "$result" == *"ping"* ]]
    [[ "$result" == *"config"* ]]
}
