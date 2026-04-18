#!/usr/bin/env bash

setup_file() {
    export BATS_TEST_TIMEOUT=30
}

common_setup() {
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
    local config="${1:-${START_SERVER_CONFIG:-tests/data/config.yaml}}"
    cargo run -- -c "$config" -n "$NAMESPACE" >&2 &
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
    9 9pfuse -A 0 -D unix!"$NAMESPACE/charlesnetfs" "$MOUNT_DIR" 2>/tmp/9pfuse.log
    for i in {1..10}; do
        mountpoint -q "$MOUNT_DIR" && return 0
        sleep 0.25
    done
    return 1
}
