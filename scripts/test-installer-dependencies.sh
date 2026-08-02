#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/gaterust-installer-dependencies.XXXXXX")
GREP=$(command -v grep)
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

die() { printf '错误：%s\n' "$*" >&2; exit 1; }

GATERUST_LIBRARY_ONLY=1
export GATERUST_LIBRARY_ONLY
. "$SCRIPT_DIR/gaterust.sh"

make_package_manager() {
    manager_path=$1
    {
        printf '%s\n' '#!/bin/sh'
        printf '%s\n' 'printf "%s\n" "$*" >> "$INSTALL_LOG"'
        printf '%s\n' 'printf "#!/bin/sh\nexit 0\n" > "$FAKE_BIN/openssl"'
        printf '%s\n' '/bin/chmod +x "$FAKE_BIN/openssl"'
    } > "$manager_path"
    chmod +x "$manager_path"
}

test_package_manager() (
    manager=$1
    expected=$2
    case_dir="$TEST_ROOT/$manager"
    FAKE_BIN="$case_dir/bin"
    INSTALL_LOG="$case_dir/install.log"
    mkdir -p "$FAKE_BIN"
    make_package_manager "$FAKE_BIN/$manager"
    export FAKE_BIN INSTALL_LOG
    PATH=$FAKE_BIN
    INTERACTIVE=0
    ASSUME_YES=1

    ensure_openssl
    "$GREP" -Fqx -- "$expected" "$INSTALL_LOG" || die "$manager 安装参数不正确"
    [ -x "$FAKE_BIN/openssl" ] || die "$manager 安装后未检测到 openssl"
)

test_requires_consent() (
    case_dir="$TEST_ROOT/requires-consent"
    FAKE_BIN="$case_dir/bin"
    INSTALL_LOG="$case_dir/install.log"
    error_log="$case_dir/error.log"
    mkdir -p "$FAKE_BIN"
    make_package_manager "$FAKE_BIN/apk"
    export FAKE_BIN INSTALL_LOG
    PATH=$FAKE_BIN
    INTERACTIVE=0
    ASSUME_YES=0

    if (ensure_openssl) 2> "$error_log"; then
        die "非交互安装不应在未经授权时安装依赖"
    fi
    [ ! -e "$INSTALL_LOG" ] || die "未经授权时调用了包管理器"
    "$GREP" -Fq -- '--yes' "$error_log" || die "缺少非交互安装授权提示"
)

test_existing_openssl() (
    case_dir="$TEST_ROOT/existing-openssl"
    FAKE_BIN="$case_dir/bin"
    INSTALL_LOG="$case_dir/install.log"
    mkdir -p "$FAKE_BIN"
    make_package_manager "$FAKE_BIN/apk"
    printf '#!/bin/sh\nexit 0\n' > "$FAKE_BIN/openssl"
    chmod +x "$FAKE_BIN/openssl"
    export FAKE_BIN INSTALL_LOG
    PATH=$FAKE_BIN
    INTERACTIVE=0
    ASSUME_YES=0

    ensure_openssl
    [ ! -e "$INSTALL_LOG" ] || die "openssl 已存在时不应调用包管理器"
)

test_package_manager apk 'add --no-cache openssl'
test_package_manager apt-get 'install -y --no-install-recommends openssl'
"$GREP" -Fqx -- 'update' "$TEST_ROOT/apt-get/install.log" || die "apt-get 安装前未更新软件包索引"
test_package_manager dnf 'install -y openssl'
test_package_manager zypper '--non-interactive install --no-recommends openssl'
test_package_manager pacman '-S --noconfirm --needed openssl'
test_requires_consent
test_existing_openssl
printf '%s\n' '安装器依赖处理测试通过。'
