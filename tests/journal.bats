#!/usr/bin/env bats

load 'test_helper/helpers'

setup_file() {
    _setup_file
}

setup() {
    export START_SERVER_CONFIG="tests/data/config-journal.yaml"
    common_setup
    start_server
    wait_for_socket
}

teardown() {
    common_teardown
}

@test "target directory lists journal file via 9p" {
    result=$(9 9p ls charlesnetfs/router/)

    [[ "$result" == *"journal"* ]]
    [[ "$result" == *"ping"* ]]
    [[ "$result" == *"config"* ]]
}

@test "journal file is visible via 9p stat" {
    result=$(9 9p stat charlesnetfs/router/journal)

    [[ "$result" == *"journal"* ]]
}

@test "journal file is read-only via 9p stat" {
    result=$(9 9p stat charlesnetfs/router/journal)

    [[ "$result" == *'m 0444'* ]]
}

@test "journal file exists in each target via 9p" {
    for target in router nas laptop; do
        result=$(9 9p ls charlesnetfs/${target}/)
        [[ "$result" == *"journal"* ]] || return 1
    done
}

@test "fuse mount shows journal file in target directory" {
    mount_fuse

    [ -f "$MOUNT_DIR/router/journal" ]
    [ -f "$MOUNT_DIR/nas/journal" ]
    [ -f "$MOUNT_DIR/laptop/journal" ]
}

@test "fuse mount journal file is read-only" {
    mount_fuse

    perms=$(stat -c %a "$MOUNT_DIR/router/journal")
    [ "$perms" = "444" ]
}

@test "fuse mount journal file is not a directory" {
    mount_fuse

    [ -f "$MOUNT_DIR/router/journal" ]
    [ ! -d "$MOUNT_DIR/router/journal" ]
}

@test "hot-reload config includes journal files for new targets" {
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
    [[ "$result" == *"journal"* ]]
    [[ "$result" == *"ping"* ]]
    [[ "$result" == *"config"* ]]
}

