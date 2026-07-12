#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
OUTPUT_ROOT=${GATERUST_LOCAL_RELEASE_DIR:-"$REPO_ROOT/dist-local"}
SERVER_HOST=${GATERUST_LOCAL_RELEASE_HOST:-127.0.0.1}
SERVER_PORT=${GATERUST_LOCAL_RELEASE_PORT:-18080}

SERVER_PID=""
SERVER_LOG=""
TEMP_DIRS=()

say() { printf '%s\n' "$*"; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }

cleanup() {
    if [[ -n "$SERVER_PID" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    local path
    for path in "${TEMP_DIRS[@]}"; do
        [[ -d "$path" ]] && rm -rf -- "$path"
    done
}
trap cleanup EXIT HUP INT TERM

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "未找到命令：$1"
}

as_root() {
    if [[ $EUID -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

detect_release() {
    local workspace_version
    SCRIPT_VERSION=$(sed -n 's/^SCRIPT_VERSION="\([^"]*\)"/\1/p' "$SCRIPT_DIR/gaterust.sh")
    workspace_version=$(awk -F'"' '/^version = "/ { print $2; exit }' "$REPO_ROOT/Cargo.toml")
    [[ -n "$SCRIPT_VERSION" ]] || die "无法读取安装脚本版本"
    [[ -n "$workspace_version" ]] || die "无法读取 workspace 版本"
    [[ "$SCRIPT_VERSION" == "v$workspace_version" ]] ||
        die "版本不一致：Cargo.toml 为 v$workspace_version，安装脚本为 $SCRIPT_VERSION"

    case "$(uname -m)" in
        x86_64) ARCH=x86_64 ;;
        aarch64 | arm64) ARCH=aarch64 ;;
        *) die "不支持的架构：$(uname -m)" ;;
    esac
    TARGET="$ARCH-unknown-linux-musl"
    RELEASE_DIR="$OUTPUT_ROOT/$SCRIPT_VERSION"
    ASSET="gaterust-$ARCH-linux-musl.tar.gz"
}

build_release() {
    require_command cargo
    require_command grep
    require_command install
    require_command pnpm
    require_command rustup
    require_command rustc
    require_command sed
    require_command sha256sum
    require_command tar
    detect_release

    local release_tmp stage musl_cc cc_env linker_env rust_version
    rust_version=$(rustc --version)
    [[ "$rust_version" == "rustc 1.97.0 "* ]] ||
        die "需要 Rust 1.97.0，当前为：$rust_version"
    if command -v "$ARCH-linux-musl-gcc" >/dev/null 2>&1; then
        musl_cc=$(command -v "$ARCH-linux-musl-gcc")
    elif command -v musl-gcc >/dev/null 2>&1; then
        musl_cc=$(command -v musl-gcc)
    else
        die "缺少 musl C 编译器；Debian/Ubuntu 请执行：sudo apt-get install musl-tools"
    fi
    mkdir -p "$OUTPUT_ROOT"
    release_tmp=$(mktemp -d "$OUTPUT_ROOT/.release-${SCRIPT_VERSION}.XXXXXX")
    TEMP_DIRS+=("$release_tmp")
    stage="$release_tmp/stage"
    mkdir -p "$stage/config" "$release_tmp/assets"

    say "构建 Web UI..."
    pnpm --dir "$REPO_ROOT/web" install --frozen-lockfile
    pnpm --dir "$REPO_ROOT/web" build

    say "构建 $TARGET 服务端..."
    if ! rustup target list --installed | grep -Fqx "$TARGET"; then
        rustup target add "$TARGET"
    fi
    cc_env="CC_${TARGET//-/_}"
    linker_env="CARGO_TARGET_${TARGET^^}_LINKER"
    linker_env=${linker_env//-/_}
    env "$cc_env=$musl_cc" "$linker_env=$musl_cc" cargo build \
        --manifest-path "$REPO_ROOT/Cargo.toml" \
        --locked \
        --release \
        -p gaterust-server \
        --all-features \
        --target "$TARGET"

    install -m 0755 \
        "$REPO_ROOT/target/$TARGET/release/gaterust-server" \
        "$stage/gaterust-server"
    cp -a "$REPO_ROOT/web/dist" "$stage/web"
    install -m 0644 "$SCRIPT_DIR/gaterust.service" "$stage/gaterust.service"
    sed \
        's#../certs/server.pem#/etc/gaterust/tunnel/server.pem#; s#../certs/server-key.pem#/etc/gaterust/tunnel/server-key.pem#' \
        "$REPO_ROOT/config/server.example.toml" > "$stage/config/server.example.toml"
    sed \
        's#../data/acme#/var/lib/gaterust/proxy/acme#' \
        "$REPO_ROOT/config/proxy.example.toml" > "$stage/config/proxy.example.toml"
    sed \
        's#../web/dist#/usr/local/lib/gaterust/web#' \
        "$REPO_ROOT/config/web.example.toml" > "$stage/config/web.example.toml"
    printf '%s\n' "$SCRIPT_VERSION" > "$stage/VERSION"
    printf '%s\n' "$TARGET" > "$stage/TARGET"

    tar -C "$stage" -czf "$release_tmp/assets/$ASSET" .
    install -m 0755 "$SCRIPT_DIR/gaterust.sh" "$release_tmp/assets/gaterust.sh"
    (
        cd "$release_tmp/assets"
        sha256sum "$ASSET" gaterust.sh > SHA256SUMS
        sha256sum -c SHA256SUMS
    )
    tar -tzf "$release_tmp/assets/$ASSET" > "$release_tmp/archive-files"
    for required_path in \
        ./gaterust-server \
        ./gaterust.service \
        ./config/server.example.toml \
        ./config/proxy.example.toml \
        ./config/web.example.toml \
        ./web/ \
        ./VERSION \
        ./TARGET; do
        grep -Fqx "$required_path" "$release_tmp/archive-files" ||
            die "发布包缺少：$required_path"
    done

    rm -rf -- "$RELEASE_DIR"
    mv "$release_tmp/assets" "$RELEASE_DIR"
    say "本地 Release 已生成：$RELEASE_DIR"
}

require_release() {
    detect_release
    [[ -f "$RELEASE_DIR/$ASSET" ]] || die "缺少 $RELEASE_DIR/$ASSET，请先执行 build"
    [[ -f "$RELEASE_DIR/gaterust.sh" ]] || die "缺少本地安装脚本，请先执行 build"
    [[ -f "$RELEASE_DIR/SHA256SUMS" ]] || die "缺少 SHA256SUMS，请先执行 build"
}

start_server() {
    require_command curl
    require_command python3
    require_release

    local log_dir url attempt
    log_dir=$(mktemp -d "${TMPDIR:-/tmp}/gaterust-local-release.XXXXXX")
    TEMP_DIRS+=("$log_dir")
    SERVER_LOG="$log_dir/http-server.log"
    python3 -m http.server "$SERVER_PORT" \
        --bind "$SERVER_HOST" \
        --directory "$OUTPUT_ROOT" >"$SERVER_LOG" 2>&1 &
    SERVER_PID=$!
    url="http://$SERVER_HOST:$SERVER_PORT/$SCRIPT_VERSION/gaterust.sh"

    for attempt in {1..50}; do
        if curl -fsS "$url" >/dev/null 2>&1; then
            say "本地 Release 地址：http://$SERVER_HOST:$SERVER_PORT/$SCRIPT_VERSION/"
            return
        fi
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            cat "$SERVER_LOG" >&2
            die "本地 HTTP 服务启动失败"
        fi
        sleep 0.1
    done
    cat "$SERVER_LOG" >&2
    die "本地 HTTP 服务未能及时响应"
}

ensure_clean_vm() {
    [[ "${GATERUST_ALLOW_EXISTING:-0}" == 1 ]] && return
    local managed_path
    for managed_path in \
        /usr/local/bin/gaterust-server \
        /usr/local/sbin/gaterust \
        /etc/gaterust \
        /etc/systemd/system/gaterust.service \
        /var/lib/gaterust \
        /usr/local/lib/gaterust; do
        if as_root test -e "$managed_path"; then
            die "检测到已有路径 $managed_path；请使用一次性 VM，或明确设置 GATERUST_ALLOW_EXISTING=1"
        fi
    done
}

verify_install() {
    local default_install=$1 expect_running=$2
    local state modules expected_state service_environment service_environment_attributes
    local attempt tunnel_mode proxy_mode
    as_root test -x /usr/local/bin/gaterust-server || die "服务端二进制未安装"
    as_root test -x /usr/local/sbin/gaterust || die "管理脚本未安装"
    as_root test -f /etc/systemd/system/gaterust.service || die "systemd unit 未安装"
    as_root test -f /var/lib/gaterust/install-state || die "安装状态未生成"
    cmp "$REPO_ROOT/target/$TARGET/release/gaterust-server" /usr/local/bin/gaterust-server >/dev/null ||
        die "安装后的服务端二进制与本地构建产物不一致"
    cmp "$RELEASE_DIR/gaterust.sh" /usr/local/sbin/gaterust >/dev/null ||
        die "安装后的管理脚本与本地 Release 不一致"

    state=$(as_root cat /var/lib/gaterust/install-state)
    expected_state=$(printf 'VERSION=%s\nARCH=%s\n' "$SCRIPT_VERSION" "$ARCH")
    [[ "$state" == "$expected_state"* ]] || die "安装状态中的版本或架构不正确"
    modules=$(sed -n 's/^MODULES=//p' <<<"$state")
    service_environment_attributes=$(stat -c '%a %U %G' /var/lib/gaterust/service.env)
    [[ "$service_environment_attributes" == "644 root root" ]] ||
        die "服务参数文件权限不正确：$service_environment_attributes"
    service_environment=$(cat /var/lib/gaterust/service.env)
    if [[ ",$modules," == *,web,* ]]; then
        as_root test -f /etc/gaterust/web/web.toml || die "Web 正式配置不存在"
    fi
    if [[ "$default_install" == 1 ]]; then
        as_root test ! -f /etc/gaterust/tunnel/server.toml || die "QUIC 示例配置不应成为正式配置"
        as_root test ! -f /etc/gaterust/proxy/proxy.toml || die "Proxy 示例配置不应成为正式配置"
        [[ "$service_environment" == *"--enable-web"* ]] || die "服务参数未启用 Web"
        [[ "$service_environment" != *"--enable-tunnel"* ]] || die "服务参数不应启用未配置的 QUIC"
        [[ "$service_environment" != *"--enable-proxy"* ]] || die "服务参数不应启用未配置的 Proxy"
        tunnel_mode=$(as_root stat -c '%a' /etc/gaterust/tunnel)
        proxy_mode=$(as_root stat -c '%a' /etc/gaterust/proxy)
        [[ "$tunnel_mode" == 770 && "$proxy_mode" == 770 ]] || die "Web 配置目录权限不正确"
        as_root grep -Fq -- '-/etc/gaterust/tunnel' /etc/systemd/system/gaterust.service ||
            die "systemd 未放行 Web 写入 QUIC 配置目录"
        as_root grep -Fq -- '-/etc/gaterust/proxy' /etc/systemd/system/gaterust.service ||
            die "systemd 未放行 Web 写入 Proxy 配置目录"
    fi
    if [[ "$expect_running" == 1 ]]; then
        as_root systemctl is-active --quiet gaterust.service || die "GateRust 服务未运行"
        if [[ "$service_environment" == *"--enable-web"* ]]; then
            for attempt in {1..50}; do
                curl -fsS http://127.0.0.1:8080/ >/dev/null && break
                sleep 0.1
            done
            curl -fsS http://127.0.0.1:8080/ >/dev/null || die "Web 管理界面未响应"
        fi
    fi

    /usr/local/sbin/gaterust status
    say "本地 Release 安装及 Web 启动验证通过。"
    say "测试完成后可执行：$0 uninstall"
}

test_release() {
    require_command systemctl
    [[ -d /run/systemd/system ]] || die "当前 VM 未运行 systemd"
    if [[ $EUID -ne 0 ]]; then
        require_command sudo
        sudo -v
    fi
    ensure_clean_vm
    build_release
    start_server

    local base_url="http://$SERVER_HOST:$SERVER_PORT" default_install=1 expect_running=1 argument
    local installer_args=(install --modules tunnel,proxy,web --enable)
    if [[ $# -gt 0 ]]; then
        [[ $1 == -- ]] || die "自定义安装参数前需要使用 --"
        shift
        installer_args=(install "$@")
        default_install=0
    fi
    if [[ "${GATERUST_ALLOW_EXISTING:-0}" == 1 ]]; then
        installer_args+=(--force)
        default_install=0
    fi
    expect_running=0
    for argument in "${installer_args[@]}"; do
        [[ "$argument" == --enable || "$argument" == --start ]] && expect_running=1
    done

    say "执行本地安装器..."
    curl -fsSL "$base_url/$SCRIPT_VERSION/gaterust.sh" |
        as_root env GATERUST_RELEASE_BASE="$base_url" sh -s -- "${installer_args[@]}"
    verify_install "$default_install" "$expect_running"
}

uninstall_release() {
    if [[ $EUID -ne 0 ]]; then
        require_command sudo
        sudo -v
    fi
    as_root test -x /usr/local/sbin/gaterust || die "未找到已安装的 GateRust 管理脚本"
    as_root /usr/local/sbin/gaterust uninstall --all --yes
}

usage() {
    cat <<EOF
用法：
  $0 build
  $0 serve
  $0 test [-- <安装器参数>]
  $0 uninstall

命令：
  build      构建当前架构的本地 Release 资产
  serve      构建资产并在本地持续提供 HTTP 下载
  test       构建、启动临时 HTTP 服务、安装并校验；默认安装全部模块
  uninstall  完整卸载测试安装

示例：
  $0 test
  $0 test -- --modules web --web-config /path/to/web.toml --enable

环境变量：
  GATERUST_LOCAL_RELEASE_DIR   资产目录，默认是仓库下的 dist-local
  GATERUST_LOCAL_RELEASE_HOST  HTTP 监听地址，默认是 127.0.0.1
  GATERUST_LOCAL_RELEASE_PORT  HTTP 端口，默认是 18080
  GATERUST_ALLOW_EXISTING=1    允许在检测到已有安装时继续，用于升级测试
EOF
}

command_name=${1:-test}
[[ $# -gt 0 ]] && shift
case "$command_name" in
    build) [[ $# -eq 0 ]] || die "build 不接受参数"; build_release ;;
    serve) [[ $# -eq 0 ]] || die "serve 不接受参数"; build_release; start_server; wait "$SERVER_PID" ;;
    test) test_release "$@" ;;
    uninstall) [[ $# -eq 0 ]] || die "uninstall 不接受参数"; uninstall_release ;;
    help | -h | --help) usage ;;
    *) usage >&2; die "未知命令：$command_name" ;;
esac
