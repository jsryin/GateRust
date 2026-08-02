#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/gaterust-installer-test.XXXXXX")
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

die() { printf '错误：%s\n' "$*" >&2; exit 1; }

assert_contains() {
    grep -Fq "$2" "$1" || die "$1 缺少：$2"
}

assert_not_contains() {
    if grep -Fq "$2" "$1"; then die "$1 不应包含：$2"; fi
}

test_service_file() (
    manager=$1
    modules=$2
    expected_capability=$3
    case_name=$(printf '%s' "$modules" | tr ',' '-')
    case_dir="$TEST_ROOT/$manager-$case_name"
    mkdir -p "$case_dir/root" "$case_dir/temp"

    GATERUST_ROOT="$case_dir/root"
    GATERUST_SERVICE_MANAGER=$manager
    GATERUST_TESTING=1
    GATERUST_LIBRARY_ONLY=1
    GATERUST_SYSTEMCTL=true
    GATERUST_RC_SERVICE=true
    GATERUST_RC_UPDATE=true
    export GATERUST_ROOT GATERUST_SERVICE_MANAGER GATERUST_TESTING GATERUST_LIBRARY_ONLY
    export GATERUST_SYSTEMCTL GATERUST_RC_SERVICE GATERUST_RC_UPDATE
    . "$SCRIPT_DIR/gaterust.sh"

    require_platform
    TEMP_DIR="$case_dir/temp"
    package=$SCRIPT_DIR
    write_service_files "$modules" "$modules"

    assert_not_contains "$TEMP_DIR/service-file" '@PROXY_CAPABILITIES@'
    assert_contains "$TEMP_DIR/service.env" 'GATERUST_ARGS='
    if [ "$expected_capability" -eq 1 ]; then
        case "$manager" in
            systemd)
                assert_contains "$TEMP_DIR/service-file" 'AmbientCapabilities=CAP_NET_BIND_SERVICE'
                assert_contains "$TEMP_DIR/service-file" 'CapabilityBoundingSet=CAP_NET_BIND_SERVICE'
                ;;
            openrc) assert_contains "$TEMP_DIR/service-file" 'capabilities="^cap_net_bind_service"' ;;
        esac
    else
        assert_not_contains "$TEMP_DIR/service-file" 'CAP_NET_BIND_SERVICE'
        assert_not_contains "$TEMP_DIR/service-file" 'cap_net_bind_service'
    fi
    if [ "$manager" = openrc ]; then
        assert_contains "$TEMP_DIR/service-file" 'supervisor="supervise-daemon"'
        assert_contains "$TEMP_DIR/service-file" 'command_user="gaterust:gaterust"'
        sh -n "$TEMP_DIR/service-file"
    fi
)

test_service_file systemd tunnel 0
test_service_file systemd tunnel,proxy,web 1
test_service_file openrc web 0
test_service_file openrc tunnel,proxy,web 1
printf '%s\n' '安装器服务文件生成测试通过。'
