#!/usr/bin/env bats

load 'test_helper/helpers'

setup() {
    common_setup
    export START_SERVER_CONFIG="tests/data/config-journal.yaml"
}

@test "target directory lists log file via 9p" {
    start_server
    wait_for_socket

    result=$(9 9p ls charlesnetfs/router/)

    [[ "$result" == *"log"* ]]
    [[ "$result" == *"ping"* ]]
    [[ "$result" == *"config"* ]]
}

@test "log file is visible via 9p stat" {
    start_server
    wait_for_socket

    result=$(9 9p stat charlesnetfs/router/log)

    [[ "$result" == *"log"* ]]
}

@test "log file is read-only via 9p stat" {
    start_server
    wait_for_socket

    result=$(9 9p stat charlesnetfs/router/log)

    [[ "$result" == *'m 0444'* ]]
}

@test "log file exists in each target via 9p" {
    start_server
    wait_for_socket

    for target in router nas laptop; do
        result=$(9 9p ls charlesnetfs/${target}/)
        [[ "$result" == *"log"* ]] || return 1
    done
}

@test "fuse mount shows log file in target directory" {
    start_server
    wait_for_socket
    mount_fuse

    [ -f "$MOUNT_DIR/router/log" ]
    [ -f "$MOUNT_DIR/nas/log" ]
    [ -f "$MOUNT_DIR/laptop/log" ]
}

@test "fuse mount log file is read-only" {
    start_server
    wait_for_socket
    mount_fuse

    perms=$(stat -c %a "$MOUNT_DIR/router/log")
    [ "$perms" = "444" ]
}

@test "fuse mount log file is not a directory" {
    start_server
    wait_for_socket
    mount_fuse

    [ -f "$MOUNT_DIR/router/log" ]
    [ ! -d "$MOUNT_DIR/router/log" ]
}

@test "hot-reload config includes log files for new targets" {
    start_server
    wait_for_socket

    result=$(9 9p ls charlesnetfs/)
    [[ "$result" == *"router"* ]]

    new_config="global_timeout_ms: 5000
journal_limit: 25
targets:
  newhost:
    addr: 10.0.0.99:22
    journal_port: 19531
"
    echo "$new_config" | 9 9p write charlesnetfs/config

    result=$(9 9p ls charlesnetfs/newhost/)
    [[ "$result" == *"log"* ]]
    [[ "$result" == *"ping"* ]]
    [[ "$result" == *"config"* ]]
}

@test "log file walk via 9p" {
    start_server
    wait_for_socket

    result=$(9 9p walk charlesnetfs/router/log 2>&1)

    [[ "$result" == *"log"* ]]
}
