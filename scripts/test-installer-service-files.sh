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

test_openrc_runtime_permissions() (
    case_dir="$TEST_ROOT/openrc-runtime"
    fake_bin="$case_dir/bin"
    chown_log="$case_dir/chown.log"
    mkdir -p "$case_dir/root" "$fake_bin"
    printf '%s\n' \
        '#!/bin/sh' \
        'printf '\''%s\n'\'' "$*" >> "$GATERUST_TEST_CHOWN_LOG"' \
        > "$fake_bin/chown"
    chmod 0755 "$fake_bin/chown"

    GATERUST_ROOT="$case_dir/root"
    GATERUST_SERVICE_MANAGER=openrc
    GATERUST_TESTING=1
    GATERUST_LIBRARY_ONLY=1
    GATERUST_RC_SERVICE=true
    GATERUST_RC_UPDATE=true
    GATERUST_TEST_CHOWN_LOG=$chown_log
    PATH="$fake_bin:$PATH"
    export GATERUST_ROOT GATERUST_SERVICE_MANAGER GATERUST_TESTING GATERUST_LIBRARY_ONLY
    export GATERUST_RC_SERVICE GATERUST_RC_UPDATE GATERUST_TEST_CHOWN_LOG PATH
    . "$SCRIPT_DIR/gaterust.sh"

    SERVICE_MANAGER=openrc
    prepare_service_runtime

    assert_contains "$chown_log" "root:gaterust $case_dir/root/var/log/gaterust"
    [ "$(stat -c '%a' "$case_dir/root/var/log/gaterust")" = 750 ] ||
        die "OpenRC 日志目录权限不正确"
    assert_contains "$chown_log" "root:gaterust $case_dir/root/var/log/gaterust/gaterust.log"
    [ "$(stat -c '%a' "$case_dir/root/var/log/gaterust/gaterust.log")" = 660 ] ||
        die "OpenRC 日志文件权限不正确"
)

test_status_detects_missing_process() (
    case_dir="$TEST_ROOT/openrc-status"
    mkdir -p "$case_dir/root"

    GATERUST_ROOT="$case_dir/root"
    GATERUST_SERVICE_MANAGER=openrc
    GATERUST_TESTING=1
    GATERUST_LIBRARY_ONLY=1
    GATERUST_RC_SERVICE=true
    GATERUST_RC_UPDATE=true
    export GATERUST_ROOT GATERUST_SERVICE_MANAGER GATERUST_TESTING GATERUST_LIBRARY_ONLY
    export GATERUST_RC_SERVICE GATERUST_RC_UPDATE
    . "$SCRIPT_DIR/gaterust.sh"

    require_platform() { :; }
    read_state() {
        STATE_VERSION=v-test
        STATE_ARCH=x86_64
        STATE_MODULES=web
    }
    read_service_modules() { SERVICE_MODULES=web; }
    service_is_active() { return 0; }
    service_is_enabled() { return 0; }
    service_main_pid() { return 1; }

    status_output=$(status_command)
    case "$status_output" in
        *"服务：启动异常"*) ;;
        *) die "OpenRC 监督进程存在但应用 PID 缺失时未报告启动异常" ;;
    esac
    case "$status_output" in
        *"服务：运行中"*) die "应用 PID 缺失时不应报告运行中" ;;
        *) ;;
    esac

    service_main_pid() { printf '%s\n' 1234; }
    service_uptime_seconds() { printf '%s\n' 65; }
    status_output=$(status_command)
    case "$status_output" in
        *"服务：运行中"*) ;;
        *) die "应用 PID 存在时未报告运行中" ;;
    esac
    case "$status_output" in
        *"PID：1234"*) ;;
        *) die "运行状态未显示应用 PID" ;;
    esac
)

test_service_file systemd tunnel 0
test_service_file systemd tunnel,proxy,web 1
test_service_file openrc web 0
test_service_file openrc tunnel,proxy,web 1
test_openrc_runtime_permissions
test_status_detects_missing_process
printf '%s\n' '安装器服务文件生成测试通过。'
