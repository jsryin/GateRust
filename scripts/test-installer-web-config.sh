#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/gaterust-installer-web-config.XXXXXX")

die() { printf '错误：%s\n' "$*" >&2; exit 1; }

assert_contains() {
    grep -Fq "$2" "$1" || die "$1 缺少：$2"
}

GATERUST_LIBRARY_ONLY=1
export GATERUST_LIBRARY_ONLY
. "$SCRIPT_DIR/gaterust.sh"
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

make_fake_package() {
    package_dir=$1
    mkdir -p "$package_dir"
    {
        printf '%s\n' '#!/bin/sh'
        printf '%s\n' 'password=$(cat)'
        printf '%s\n' 'printf '\''test-hash-%s\n'\'' "$password"'
    } > "$package_dir/gaterust-server"
    chmod 0755 "$package_dir/gaterust-server"
}

use_deterministic_random() {
    random_hex() {
        case "$1" in
            16) printf '%s' 'automatic-password' ;;
            32) printf '%s' 'jwt-secret' ;;
            *) die "未预期的随机字节数：$1" ;;
        esac
    }
}

test_custom_password() (
    case_dir="$TEST_ROOT/custom"
    TEMP_DIR="$case_dir/temp"
    package="$case_dir/package"
    mkdir -p "$TEMP_DIR"
    make_fake_package "$package"
    use_deterministic_random
    secret_read_count=0
    tty_read_secret() {
        secret_read_count=$((secret_read_count + 1))
        REPLY='custom-password'
    }

    generate_web_config_interactively

    [ "$secret_read_count" -eq 2 ] || die "自定义密码未要求二次确认"
    [ -z "$GENERATED_WEB_PASSWORD" ] || die "自定义密码不应作为自动密码显示"
    assert_contains "$GENERATED_WEB_CONFIG" 'admin_password_hash = "test-hash-custom-password"'
    assert_contains "$GENERATED_WEB_CONFIG" 'jwt_secret = "jwt-secret"'
)

test_automatic_password() (
    case_dir="$TEST_ROOT/automatic"
    TEMP_DIR="$case_dir/temp"
    package="$case_dir/package"
    mkdir -p "$TEMP_DIR"
    make_fake_package "$package"
    use_deterministic_random
    secret_read_count=0
    tty_read_secret() {
        secret_read_count=$((secret_read_count + 1))
        REPLY=""
    }

    generate_web_config_interactively

    [ "$secret_read_count" -eq 1 ] || die "留空密码时不应要求二次确认"
    [ "$GENERATED_WEB_PASSWORD" = automatic-password ] || die "留空密码时未自动生成密码"
    assert_contains "$GENERATED_WEB_CONFIG" 'admin_password_hash = "test-hash-automatic-password"'
)

test_mismatched_passwords() (
    case_dir="$TEST_ROOT/mismatch"
    TEMP_DIR="$case_dir/temp"
    package="$case_dir/package"
    error_log="$case_dir/error.log"
    mkdir -p "$TEMP_DIR"
    make_fake_package "$package"
    use_deterministic_random
    secret_read_count=0
    tty_read_secret() {
        secret_read_count=$((secret_read_count + 1))
        case "$secret_read_count" in
            1) REPLY='first-password' ;;
            2) REPLY='second-password' ;;
        esac
    }

    if (generate_web_config_interactively) 2> "$error_log"; then
        die "两次输入的密码不一致时不应生成配置"
    fi
    assert_contains "$error_log" '两次输入的 Web 管理员密码不一致'
    [ ! -e "$TEMP_DIR/web.toml" ] || die "密码不一致时生成了 Web 配置"
)

test_custom_password
test_automatic_password
test_mismatched_passwords
printf '%s\n' '安装器 Web 配置初始化测试通过。'
