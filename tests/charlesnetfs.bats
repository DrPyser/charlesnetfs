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
    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}

@test "fuse mount shows target directories" {
    start_server
    wait_for_socket
    mount_fuse

    [ -d "$MOUNT_DIR/router" ]
    [ -d "$MOUNT_DIR/nas" ]
    [ -d "$MOUNT_DIR/laptop" ]
}

@test "fuse mount shows global config file" {
    start_server
    wait_for_socket
    mount_fuse

    [ -f "$MOUNT_DIR/config" ]
}

@test "fuse mount shows target config files" {
    start_server
    wait_for_socket
    mount_fuse

    [ -f "$MOUNT_DIR/router/config" ]
    [ -f "$MOUNT_DIR/nas/config" ]
}

@test "fuse mount target config is read-only" {
    start_server
    wait_for_socket
    mount_fuse

    perms=$(stat -c %a "$MOUNT_DIR/router/config")
    [ "$perms" = "444" ]
}

@test "fuse mount global config is writable" {
    start_server
    wait_for_socket
    mount_fuse

    perms=$(stat -c %a "$MOUNT_DIR/config")
    [ "$perms" = "644" ]
}

@test "fuse mount can read global config" {
    start_server
    wait_for_socket
    mount_fuse

    result=$(cat "$MOUNT_DIR/config")
    [[ "$result" == *"global_timeout_ms:"* ]]
}

@test "fuse mount can update global config" {
    start_server
    wait_for_socket
    mount_fuse

    [ -d "$MOUNT_DIR/router" ]

    cat > "$MOUNT_DIR/config" <<EOF
global_timeout_ms: 2000
targets:
  newhost:
    addr: 10.0.0.99:22
EOF

    result=$(cat "$MOUNT_DIR/config")
    [[ "$result" == *"global_timeout_ms: 2000"* ]]
    [[ "$result" == *"newhost"* ]]

    [ -d "$MOUNT_DIR/newhost" ]
    [ ! -d "$MOUNT_DIR/router" ]
}

@test "TERM signal shuts down server cleanly" {
    start_server
    wait_for_socket

    kill -TERM $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}

@test "INT signal during fuse mount cleans up properly" {
    start_server
    wait_for_socket
    mount_fuse

    kill -INT $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2

    [ ! -e "$NAMESPACE/charlesnetfs" ]

    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    fi
}

@test "TERM signal during fuse mount cleans up properly" {
    start_server
    wait_for_socket
    mount_fuse

    kill -TERM $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2

    [ ! -e "$NAMESPACE/charlesnetfs" ]

    if mountpoint -q "$MOUNT_DIR" 2>/dev/null; then
        fusermount -u "$MOUNT_DIR" 2>/dev/null || true
    fi
}

@test "TERM signal handles rapid successive signals" {
    start_server
    wait_for_socket

    kill -TERM $(cat "$TMPDIR/charlesnetfs.pid")
    sleep 0.2

    if ps -p $(cat "$TMPDIR/charlesnetfs.pid") >&2; then
        kill -TERM $(cat "$TMPDIR/charlesnetfs.pid")
        sleep 0.2
    fi

    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2 || true

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}

@test "INT signal with no active connections cleans up" {
    start_server
    wait_for_socket

    kill -INT $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}

@test "TERM signal with no active connections cleans up" {
    start_server
    wait_for_socket

    kill -TERM $(cat "$TMPDIR/charlesnetfs.pid")
    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}

@test "INT then TERM signal sequence handles gracefully" {
    start_server
    wait_for_socket

    kill -INT $(cat "$TMPDIR/charlesnetfs.pid")
    sleep 0.2

    if ps -p $(cat "$TMPDIR/charlesnetfs.pid") >&2; then
        kill -TERM $(cat "$TMPDIR/charlesnetfs.pid")
    fi

    wait $(cat "$TMPDIR/charlesnetfs.pid") >&2 || true

    [ ! -e "$NAMESPACE/charlesnetfs" ]
}
