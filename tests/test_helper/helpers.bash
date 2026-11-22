#!/usr/bin/env bash

_setup_file() {
    export BATS_TEST_TIMEOUT=30
    # warmup cargo run build
    cargo build --bin charlesnetfs
    [ -f $BATS_TEST_DIRNAME/../target/debug/charlesnetfs ] || exit 1
    export CHARLESNETFS=$(realpath $BATS_TEST_DIRNAME/../target/debug/charlesnetfs)
}

common_setup() {
    export NAMESPACE="$BATS_TEST_TMPDIR/ns"
    mkdir -p "$NAMESPACE"
    export MOUNT_DIR="$BATS_TEST_TMPDIR/mnt"
    mkdir -p "$MOUNT_DIR"
}

common_teardown() {
    local pidfile pid

    stop_server || true
    sleep 0.5
    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    fi

    # terminate any process registered in pid files
    for pidfile in $(find $BATS_TEST_TMPDIR -name '*.pid'); do
        pid=$(cat "$pidfile")
        for sig in INT TERM KILL; do
            kill -0 $pid || break
            kill -$sig $pid >&2
            sleep 0.5 &
            wait -n $! $pid
        done
    done
}

start_server() {
    local config="${1:-${START_SERVER_CONFIG:-tests/data/config.yaml}}"
    export RUST_LOG=debug
    $CHARLESNETFS -c "$config" -n "$NAMESPACE" >&2 &
    SERVER_PID=$!
    echo $SERVER_PID >&2
    jobs >&2
    echo $SERVER_PID > "$BATS_TEST_TMPDIR/charlesnetfs.pid"
}

stop_server() {
    if [ -f "$BATS_TEST_TMPDIR/charlesnetfs.pid" ]; then
        for sig in INT TERM KILL; do
            if ps -p $(cat "$BATS_TEST_TMPDIR/charlesnetfs.pid"); then
                kill -$sig $(cat "$BATS_TEST_TMPDIR/charlesnetfs.pid") >&2
                sleep 0.5 &
                wait -n $! $(cat "$BATS_TEST_TMPDIR/charlesnetfs.pid")
            else
                break
            fi
        done
        rm -f "$BATS_TEST_TMPDIR/charlesnetfs.pid"
    fi
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

JOURNALD_IMAGE="charlesnetfs-journald-gateway"

start_journald_gateway() {
    local container="${1:-charlesnetfs-journald}"

    if ! command -v docker >/dev/null 2>&1; then
        return 1
    fi

    docker rm -f "$container" >/dev/null 2>&1 || true
    docker build -f tests/data/journald-gateway/Dockerfile -t "$JOURNALD_IMAGE" .
    docker run -d --name "$container" -p 19531 "$JOURNALD_IMAGE"
    export GATEWAY_PORT=$(docker port "$container" 19531/tcp | cut -d':' -f2)

    for i in {1..10}; do
        if curl -s http://localhost:$GATEWAY_PORT/entries >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

stop_journald_gateway() {
    local container="${1:-charlesnetfs-journald}"
    docker stop "$container" >/dev/null 2>&1 || true
    docker rm -f "$container" >/dev/null 2>&1 || true
}

wait_for() {
    local file="$1"
    local pattern="$2"
    local timeout="${3:-1}"

    timeout "$timeout" grep -m 1 "$pattern" <(tail -f "$file")
}

available_tcp_port(){
    comm -23 <(seq 10000 65535 | sort) <(ss -tln | awk '{print $4}' | grep -oP '\d+$' | sort -u) | head -1
}

start_tcp_listener() {
    # find an open port
    local listener_pid port=$1
    9 listen1 -v 'tcp!localhost!'$port echo hello >&2 3<&- &
    listener_pid=$!
    echo "started listener on port $port pid=$listener_pid" >&2
    echo $listener_pid > "$BATS_TEST_TMPDIR/listener.pid"
    while ! ss -tln | grep -q ":$port"; do sleep 0.25; done
}

