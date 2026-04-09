#!/usr/bin/env bats

setup_file() {
    export BATS_TEST_TIMEOUT=30
}

setup() {
    SOCKET_PATH="/tmp/ns.${USER}.:0/charlesnetfs"
    rm -f "$SOCKET_PATH"
    export TMPDIR="${TMPDIR:-/tmp}"
}

teardown() {
    if [ -f "$TMPDIR/charlesnetfs.pid" ]; then
        kill -INT $(cat "$TMPDIR/charlesnetfs.pid") 2>/dev/null || true
        rm -f "$TMPDIR/charlesnetfs.pid"
    fi
    sleep 0.5
    rm -f "$SOCKET_PATH"
}

start_server() {
    cargo run -- -c config.yaml > /dev/null 2>&1 &
    SERVER_PID=$!
    echo $SERVER_PID > "$TMPDIR/charlesnetfs.pid"

    for i in {1..20}; do
        [ -S "$SOCKET_PATH" ] && return 0
        sleep 0.25
    done
    return 1
}

wait_for_socket() {
    for i in {1..10}; do
        [ -S "$SOCKET_PATH" ] && return 0
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
    [ -S "$SOCKET_PATH" ]
}

@test "lists configured targets" {
    start_server
    wait_for_socket

    result=$(9 9p -a unix!$SOCKET_PATH ls)

    [[ "$result" == *"router"* ]]
    [[ "$result" == *"nas"* ]]
    [[ "$result" == *"laptop"* ]]
}

@test "lists target directory contents" {
    start_server
    wait_for_socket

    result=$(9 9p -a unix!$SOCKET_PATH ls router/)

    [[ "$result" == *"config"* ]]
    [[ "$result" == *"ping"* ]]
}

@test "reads config as yaml block" {
    start_server
    wait_for_socket

    result=$(9 9p -a unix!$SOCKET_PATH read router/config)

    [[ "$result" == *"addr:"* ]]
    [[ "$result" == *"timeout:"* ]]
}

@test "server shutdown cleans up socket" {
    start_server
    wait_for_socket

    kill -INT $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") 2>/dev/null

    [ ! -e "$SOCKET_PATH" ]
}
