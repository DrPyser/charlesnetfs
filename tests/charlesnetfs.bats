#!/usr/bin/env bats

setup_file() {
    export BATS_TEST_TIMEOUT=30
}

setup() {
    TESTS_TMPDIR=$(mktemp -d /tmp/charlesnetfs-tests-XXXXXX)
    export NAMESPACE="$TESTS_TMPDIR/ns"
    mkdir -p "$NAMESPACE"
    export MOUNT_DIR="$TESTS_TMPDIR/mnt"
    mkdir -p "$MOUNT_DIR"
    export TMPDIR="${TMPDIR:-/tmp}"
}

teardown() {
    if [ -f "$TMPDIR/charlesnetfs.pid" ]; then
        for sig in INT TERM KILL; do
            if ps -p $(cat "$TMPDIR/charlesnetfs.pid"); then
                kill -$sig $(cat "$TMPDIR/charlesnetfs.pid") >&2
                sleep 0.5 &
                wait -n $! $(cat "$TMPDIR/charlesnetfs.pid")
            else
                break
            fi
        done
        rm -f "$TMPDIR/charlesnetfs.pid"
    fi
    sleep 0.5
    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    fi
    rm -rf "$TESTS_TMPDIR"
}

start_server() {
    cargo run -- -c config.yaml -n "$NAMESPACE" >&2 &
    SERVER_PID=$!
    echo $SERVER_PID >&2
    jobs >&2
    echo $SERVER_PID > "$TMPDIR/charlesnetfs.pid"
}

wait_for_socket() {
    for i in {1..20}; do
        test -S "$NAMESPACE/charlesnetfs" && return 0
        echo "No socket at path $NAMESPACE/charlesnetfs" >&2
        ls -l "$NAMESPACE" >&2
        sleep 0.25
    done
    return 1
}

mount_fuse() {
    # -A 0 ensures no fuse caching, important for hot-reload
    9 9pfuse -A 0 -D unix!"$NAMESPACE/charlesnetfs" "$MOUNT_DIR" 2>/tmp/9pfuse.log
    for i in {1..10}; do
        mountpoint -q "$MOUNT_DIR" && return 0
        sleep 0.25
    done
    return 1
}

@test "server starts with default args" {
    start_server
    kill -0 $(cat "$TMPDIR/charlesnetfs.pid")
}

@test "socket file is created" {
    start_server
    wait_for_socket
    [ -S "$NAMESPACE/charlesnetfs" ]
}

@test "lists configured targets" {
    start_server
    wait_for_socket

    result=$(9 9p ls charlesnetfs/)

    [[ "$result" == *"router"* ]]
    [[ "$result" == *"nas"* ]]
    [[ "$result" == *"laptop"* ]]
}

@test "lists target directory contents" {
    start_server
    wait_for_socket

    result=$(9 9p ls charlesnetfs/router/)

    [[ "$result" == *"config"* ]]
    [[ "$result" == *"ping"* ]]
}

@test "reads config as yaml block" {
    start_server
    wait_for_socket

    result=$(9 9p read charlesnetfs/router/config)

    [[ "$result" == *"addr:"* ]]
    [[ "$result" == *"timeout:"* ]]
}

@test "reads root config file" {
    start_server
    wait_for_socket

    result=$(9 9p read charlesnetfs/config)

    [[ "$result" == *"global_timeout_ms:"* ]]
    [[ "$result" == *"router:"* ]]
}

@test "global config file is writable" {
    start_server
    wait_for_socket

    result=$(9 9p stat charlesnetfs/config)

    [[ "$result" == *'m 0644'* ]]
}

@test "target config file is read-only" {
    start_server
    wait_for_socket

    result=$(9 9p stat charlesnetfs/router/config)

    [[ "$result" == *'m 0444'* ]]
}

@test "hot-reload config via write" {
    start_server
    wait_for_socket

    result=$(9 9p ls charlesnetfs/)
    [[ "$result" == *"router"* ]]
    [[ "$result" == *"nas"* ]]
    [[ "$result" == *"laptop"* ]]

    new_config="global_timeout_ms: 3000
targets:
  newhost:
    addr: 10.0.0.99:22
"
    echo "$new_config" | 9 9p write charlesnetfs/config

    result=$(9 9p ls charlesnetfs/)
    [[ "$result" == *"newhost"* ]]
    [[ "$result" != *"router"* ]]
    [[ "$result" != *"nas"* ]]
    [[ "$result" != *"laptop"* ]]
}

@test "server shutdown cleans up socket" {
    start_server
    wait_for_socket

    kill -INT $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") 2>/dev/null

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}
