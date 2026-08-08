#!/bin/sh

set -eu

SCRIPT_VERSION="v0.1.1-beta.2"
REPOSITORY="jsryin/GateRust"
ROOT="${GATERUST_ROOT:-}"
SYSTEMCTL="${GATERUST_SYSTEMCTL:-systemctl}"
RC_SERVICE="${GATERUST_RC_SERVICE:-rc-service}"
RC_UPDATE="${GATERUST_RC_UPDATE:-rc-update}"
SERVICE_MANAGER="${GATERUST_SERVICE_MANAGER:-}"
RELEASE_BASE="${GATERUST_RELEASE_BASE:-https://github.com/$REPOSITORY/releases/download}"

BIN="$ROOT/usr/local/bin/gaterust-server"
CTL="$ROOT/usr/local/sbin/gaterust"
LEGACY_CTL="${CTL}ctl"
LIB_DIR="$ROOT/usr/local/lib/gaterust"
ETC_DIR="$ROOT/etc/gaterust"
DATA_DIR="$ROOT/var/lib/gaterust"
TUNNEL_DIR="$ETC_DIR/tunnel"
TUNNEL_CONFIG="$TUNNEL_DIR/server.toml"
TUNNEL_CERTIFICATE="$TUNNEL_DIR/server.pem"
TUNNEL_PRIVATE_KEY="$TUNNEL_DIR/server-key.pem"
PROXY_DIR="$ETC_DIR/proxy"
PROXY_CONFIG="$PROXY_DIR/proxy.toml"
STATE_FILE="$DATA_DIR/install-state"
ENV_FILE="$DATA_DIR/service.env"
SYSTEMD_SERVICE_FILE="$ROOT/etc/systemd/system/gaterust.service"
OPENRC_SERVICE_FILE="$ROOT/etc/init.d/gaterust"
LOG_DIR="$ROOT/var/log/gaterust"
LOG_FILE="$LOG_DIR/gaterust.log"
LOCK_DIR="$ROOT/run/lock/gaterust.lock"
SERVICE_FILE=""
SERVICE_FILE_MODE=""

TEMP_DIR=""
LOCK_HELD=0
TRANSACTION=0
TTY_STATE=""
STATE_VERSION=""
STATE_ARCH=""
STATE_MODULES=""
NORMALIZED=""

say() { printf '%s\n' "$*"; }
warn() { printf '警告：%s\n' "$*" >&2; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }

cleanup_generated_files() {
    if [ "${GENERATED_TUNNEL_FILES_INSTALLED:-0}" -eq 1 ]; then
        rm -f "$TUNNEL_CONFIG" "$TUNNEL_CERTIFICATE" "$TUNNEL_PRIVATE_KEY"
        GENERATED_TUNNEL_FILES_INSTALLED=0
    fi
    if [ "${GENERATED_PROXY_CONFIG_INSTALLED:-0}" -eq 1 ]; then
        rm -f "$PROXY_CONFIG"
        GENERATED_PROXY_CONFIG_INSTALLED=0
    fi
}

restore_tty() {
    [ -n "$TTY_STATE" ] || return 0
    if [ -r /dev/tty ]; then
        stty "$TTY_STATE" < /dev/tty 2>/dev/null || true
    fi
    TTY_STATE=""
}

cleanup() {
    restore_tty
    if [ "$TRANSACTION" -eq 1 ]; then
        rollback_install
    fi
    cleanup_generated_files
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf "$TEMP_DIR"
    fi
    if [ "$LOCK_HELD" -eq 1 ]; then
        rmdir "$LOCK_DIR" 2>/dev/null || true
    fi
}
trap cleanup EXIT HUP INT TERM

require_root() {
    [ "$(id -u)" -eq 0 ] || die "此操作需要管理员权限，请使用 sudo"
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "未找到命令：$1"
}

ensure_openssl() {
    command -v openssl >/dev/null 2>&1 && return

    if [ "$INTERACTIVE" -eq 1 ]; then
        tty_read "未检测到 openssl，是否使用系统包管理器自动安装？[Y/n]："
        case "${REPLY:-y}" in
            y|Y|yes|YES) ;;
            n|N|no|NO) die "自动初始化 QUIC 需要 openssl，请安装后重试" ;;
            *) die "无效选择" ;;
        esac
    elif [ "$ASSUME_YES" -ne 1 ]; then
        die "自动初始化 QUIC 需要 openssl；请先安装，或添加 --yes 允许自动安装依赖"
    fi

    say "正在使用系统包管理器安装 openssl..."
    if command -v apk >/dev/null 2>&1; then
        apk add --no-cache openssl || die "使用 apk 安装 openssl 失败，请手动安装后重试"
    elif command -v apt-get >/dev/null 2>&1; then
        apt-get update || die "使用 apt-get 更新软件包索引失败，请手动安装 openssl 后重试"
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssl ||
            die "使用 apt-get 安装 openssl 失败，请手动安装后重试"
    elif command -v dnf >/dev/null 2>&1; then
        dnf install -y openssl || die "使用 dnf 安装 openssl 失败，请手动安装后重试"
    elif command -v microdnf >/dev/null 2>&1; then
        microdnf install -y openssl || die "使用 microdnf 安装 openssl 失败，请手动安装后重试"
    elif command -v yum >/dev/null 2>&1; then
        yum install -y openssl || die "使用 yum 安装 openssl 失败，请手动安装后重试"
    elif command -v zypper >/dev/null 2>&1; then
        zypper --non-interactive install --no-recommends openssl ||
            die "使用 zypper 安装 openssl 失败，请手动安装后重试"
    elif command -v pacman >/dev/null 2>&1; then
        pacman -S --noconfirm --needed openssl ||
            die "使用 pacman 安装 openssl 失败，请手动安装后重试"
    else
        die "未找到受支持的包管理器，请手动安装 openssl 后重试"
    fi
    command -v openssl >/dev/null 2>&1 || die "openssl 安装完成后仍不可用，请检查 PATH 后重试"
}

run_installed_as_root() {
    [ -z "$ROOT" ] || die "测试根目录模式不支持自动提权"
    trusted_ctl=/usr/local/sbin/gaterust
    [ -x "$trusted_ctl" ] || die "未找到已安装的 GateRust 管理程序：$trusted_ctl"
    command -v sudo >/dev/null 2>&1 || die "此操作需要管理员权限，但未找到 sudo"
    sudo -- "$trusted_ctl" "$@"
}

detect_service_manager() {
    if [ -z "$SERVICE_MANAGER" ]; then
        if [ -d "$ROOT/run/systemd/system" ]; then
            SERVICE_MANAGER=systemd
        elif [ -e "$ROOT/run/openrc/softlevel" ]; then
            SERVICE_MANAGER=openrc
        else
            die "当前系统未运行受支持的服务管理器（systemd 或 OpenRC）"
        fi
    fi
    case "$SERVICE_MANAGER" in
        systemd)
            require_command "$SYSTEMCTL"
            [ -d "$ROOT/run/systemd/system" ] || [ -n "${GATERUST_TESTING:-}" ] || die "当前系统未运行 systemd"
            SERVICE_FILE=$SYSTEMD_SERVICE_FILE
            SERVICE_FILE_MODE=0644
            ;;
        openrc)
            require_command "$RC_SERVICE"
            require_command "$RC_UPDATE"
            [ -n "${GATERUST_TESTING:-}" ] || require_command supervise-daemon
            require_command pgrep
            [ -e "$ROOT/run/openrc/softlevel" ] || [ -n "${GATERUST_TESTING:-}" ] || die "当前系统未运行 OpenRC"
            SERVICE_FILE=$OPENRC_SERVICE_FILE
            SERVICE_FILE_MODE=0755
            ;;
        *) die "不支持的服务管理器：$SERVICE_MANAGER" ;;
    esac
}

require_platform() {
    [ "$(uname -s)" = "Linux" ] || die "仅支持 Linux"
    detect_service_manager
    case "$(uname -m)" in
        x86_64) ARCH="x86_64"; TARGET="x86_64-unknown-linux-musl" ;;
        aarch64|arm64) ARCH="aarch64"; TARGET="aarch64-unknown-linux-musl" ;;
        *) die "不支持的架构：$(uname -m)" ;;
    esac
}

service_is_active() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" is-active --quiet gaterust.service ;;
        openrc) "$RC_SERVICE" gaterust status >/dev/null 2>&1 ;;
    esac
}

service_is_enabled() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" is-enabled --quiet gaterust.service ;;
        openrc) [ -L "$ROOT/etc/runlevels/default/gaterust" ] ;;
    esac
}

service_start() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" start gaterust.service ;;
        openrc) "$RC_SERVICE" gaterust start ;;
    esac
}

service_stop() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" stop gaterust.service ;;
        openrc) "$RC_SERVICE" gaterust stop ;;
    esac
}

service_restart() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" restart gaterust.service ;;
        openrc) "$RC_SERVICE" gaterust restart ;;
    esac
}

service_enable() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" enable gaterust.service ;;
        openrc) "$RC_UPDATE" add gaterust default ;;
    esac
}

service_disable() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" disable gaterust.service ;;
        openrc) "$RC_UPDATE" del gaterust default ;;
    esac
}

service_reload_manager() {
    [ "$SERVICE_MANAGER" != systemd ] || "$SYSTEMCTL" daemon-reload
}

service_reset_failed() {
    [ "$SERVICE_MANAGER" != systemd ] || "$SYSTEMCTL" reset-failed gaterust.service
}

service_main_pid() {
    case "$SERVICE_MANAGER" in
        systemd) "$SYSTEMCTL" show gaterust.service -p MainPID --value 2>/dev/null ;;
        openrc)
            pgrep -o -u gaterust -x gaterust-server
            ;;
    esac
}

service_uptime_seconds() {
    service_uptime_pid=$1
    boot_seconds=$(awk '{ print int($1) }' /proc/uptime 2>/dev/null) || return 1
    case "$SERVICE_MANAGER" in
        systemd)
            service_started=$("$SYSTEMCTL" show gaterust.service -p ActiveEnterTimestampMonotonic --value 2>/dev/null) || return 1
            [ -n "$service_started" ] && [ "$service_started" -gt 0 ] 2>/dev/null || return 1
            service_seconds=$((boot_seconds - service_started / 1000000))
            ;;
        openrc)
            command -v getconf >/dev/null 2>&1 || return 1
            service_clock_ticks=$(getconf CLK_TCK) || return 1
            service_started=$(awk '{ print $22 }' "/proc/$service_uptime_pid/stat" 2>/dev/null) || return 1
            case "$service_clock_ticks:$service_started" in *[!0-9:]*) return 1 ;; esac
            [ "$service_clock_ticks" -gt 0 ] || return 1
            service_seconds=$((boot_seconds - service_started / service_clock_ticks))
            ;;
    esac
    [ "$service_seconds" -ge 0 ] || service_seconds=0
    printf '%s\n' "$service_seconds"
}

service_logs() {
    case "$SERVICE_MANAGER" in
        systemd)
            require_command journalctl
            exec journalctl -u gaterust.service -f
            ;;
        openrc)
            require_command tail
            [ -f "$LOG_FILE" ] || die "服务日志不存在：$LOG_FILE"
            exec tail -n 100 -f "$LOG_FILE"
            ;;
    esac
}

acquire_lock() {
    mkdir -p "$(dirname "$LOCK_DIR")"
    mkdir "$LOCK_DIR" 2>/dev/null || die "另一个 GateRust 管理操作正在执行"
    LOCK_HELD=1
    TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/gaterust.XXXXXX")"
}

release_lock() {
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf "$TEMP_DIR"
    fi
    TEMP_DIR=""
    if [ "$LOCK_HELD" -eq 1 ]; then
        rmdir "$LOCK_DIR" 2>/dev/null || true
        LOCK_HELD=0
    fi
}

has_module() {
    case ",${1:-}," in *",$2,"*) return 0 ;; *) return 1 ;; esac
}

display_modules() {
    display_result=""
    for display_module in tunnel proxy web; do
        has_module "$1" "$display_module" || continue
        case "$display_module" in tunnel) display_name="QUIC" ;; proxy) display_name="Proxy" ;; web) display_name="Web" ;; esac
        display_result="${display_result:+$display_result、}$display_name"
    done
    [ -n "$display_result" ] || display_result="无"
    printf '%s\n' "$display_result"
}

normalize_modules() {
    NORMALIZED=""
    old_ifs=$IFS
    IFS=,
    set -- $1
    IFS=$old_ifs
    for module_value in "$@"; do
        module_value=$(printf '%s' "$module_value" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        case "$module_value" in tunnel|proxy|web) ;; *) die "未知模块：$module_value" ;; esac
        if ! has_module "$NORMALIZED" "$module_value"; then
            NORMALIZED="${NORMALIZED:+$NORMALIZED,}$module_value"
        fi
    done
    [ -n "$NORMALIZED" ] || die "至少选择一个模块"
}

merge_modules() {
    merge_result=$1
    old_ifs=$IFS
    IFS=,
    set -- $2
    IFS=$old_ifs
    for merge_item in "$@"; do
        if ! has_module "$merge_result" "$merge_item"; then
            merge_result="${merge_result:+$merge_result,}$merge_item"
        fi
    done
    NORMALIZED=$merge_result
}

remove_modules() {
    remove_result=""
    old_ifs=$IFS
    IFS=,
    set -- $1
    IFS=$old_ifs
    for remove_item in "$@"; do
        if ! has_module "$2" "$remove_item"; then
            remove_result="${remove_result:+$remove_result,}$remove_item"
        fi
    done
    NORMALIZED=$remove_result
}

read_state() {
    STATE_VERSION=""
    STATE_ARCH=""
    STATE_MODULES=""
    [ -f "$STATE_FILE" ] || return 1
    state_seen_version=0 state_seen_arch=0 state_seen_modules=0
    while IFS='=' read -r state_key state_value; do
        case "$state_key" in
            VERSION) [ "$state_seen_version" -eq 0 ] || die "安装状态包含重复 VERSION"; STATE_VERSION=$state_value; state_seen_version=1 ;;
            ARCH) [ "$state_seen_arch" -eq 0 ] || die "安装状态包含重复 ARCH"; STATE_ARCH=$state_value; state_seen_arch=1 ;;
            MODULES) [ "$state_seen_modules" -eq 0 ] || die "安装状态包含重复 MODULES"; STATE_MODULES=$state_value; state_seen_modules=1 ;;
            '') ;;
            *) die "安装状态包含未知字段：$state_key" ;;
        esac
    done < "$STATE_FILE"
    [ -n "$STATE_VERSION" ] && [ -n "$STATE_ARCH" ] && [ -n "$STATE_MODULES" ] || die "安装状态不完整"
    case "$STATE_VERSION" in v[0-9]*.[0-9]*.[0-9]*) ;; *) die "安装状态中的版本无效" ;; esac
    case "$STATE_ARCH" in x86_64|aarch64) ;; *) die "安装状态中的架构无效" ;; esac
    normalize_modules "$STATE_MODULES"
    [ "$NORMALIZED" = "$STATE_MODULES" ] || die "安装状态中的模块列表无效"
}

fetch() {
    fetch_url=$1
    fetch_dest=$2
    if command -v curl >/dev/null 2>&1; then
        curl -fL --retry 3 --connect-timeout 15 -o "$fetch_dest" "$fetch_url"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$fetch_dest" "$fetch_url"
    else
        die "需要 curl 或 wget"
    fi
}

checksum_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "需要 sha256sum 或 shasum"
    fi
}

verify_checksum() {
    checksum_name=$(basename "$1")
    checksum_expected=$(awk -v name="$checksum_name" '$2 == name || $2 == "*" name { print $1; found = 1 } END { if (!found) exit 1 }' "$TEMP_DIR/SHA256SUMS") || die "SHA256SUMS 缺少 $checksum_name"
    checksum_actual=$(checksum_file "$1")
    [ "$checksum_actual" = "$checksum_expected" ] || die "$checksum_name 的 SHA-256 校验失败"
}

prepare_release() {
    asset="gaterust-$ARCH-linux-musl.tar.gz"
    release_url="$RELEASE_BASE/$SCRIPT_VERSION"
    fetch "$release_url/$asset" "$TEMP_DIR/$asset"
    fetch "$release_url/SHA256SUMS" "$TEMP_DIR/SHA256SUMS"
    fetch "$release_url/gaterust.sh" "$TEMP_DIR/gaterust.sh"
    verify_checksum "$TEMP_DIR/$asset"
    verify_checksum "$TEMP_DIR/gaterust.sh"
    grep -Fqx "SCRIPT_VERSION=\"$SCRIPT_VERSION\"" "$TEMP_DIR/gaterust.sh" || die "下载脚本版本与当前脚本不一致"
    mkdir "$TEMP_DIR/package"
    tar -tzf "$TEMP_DIR/$asset" | while IFS= read -r archive_path; do
        case "$archive_path" in /*|../*|*/../*|*/..) exit 1 ;; esac
    done || die "压缩包包含不安全路径"
    tar -xzf "$TEMP_DIR/$asset" -C "$TEMP_DIR/package"
    package="$TEMP_DIR/package"
    [ -x "$package/gaterust-server" ] || die "压缩包缺少 gaterust-server"
    [ -f "$package/gaterust.service" ] || die "压缩包缺少 systemd unit"
    [ -f "$package/gaterust.openrc" ] || die "压缩包缺少 OpenRC 服务脚本"
    for package_config in server.example.toml proxy.example.toml web.example.toml; do
        [ -f "$package/config/$package_config" ] || die "压缩包缺少 $package_config"
    done
    [ -d "$package/web" ] || die "压缩包缺少 Web 静态文件"
    [ "$(sed -n '1p' "$package/VERSION")" = "$SCRIPT_VERSION" ] || die "压缩包版本不匹配"
    [ "$(sed -n '1p' "$package/TARGET")" = "$TARGET" ] || die "压缩包目标架构不匹配"
}

module_config() {
    case "$1" in
        tunnel) MODULE_CONFIG=$TUNNEL_CONFIG; MODULE_EXAMPLE="server.example.toml" ;;
        proxy) MODULE_CONFIG=$PROXY_CONFIG; MODULE_EXAMPLE="proxy.example.toml" ;;
        web) MODULE_CONFIG="$ETC_DIR/web/web.toml"; MODULE_EXAMPLE="web.example.toml" ;;
    esac
}

check_configs_with() {
    check_binary=$1
    check_modules=$2
    shift 2
    set -- "$check_binary" check-config
    check_missing=0
    for check_module in tunnel proxy web; do
        if has_module "$check_modules" "$check_module"; then
            module_config "$check_module"
            check_path=$MODULE_CONFIG
            case "$check_module" in
                tunnel) check_name="QUIC"; [ -n "${TUNNEL_SOURCE:-}" ] && check_path=$TUNNEL_SOURCE; set -- "$@" --enable-tunnel --tunnel-config "$check_path" ;;
                proxy) check_name="Proxy"; [ -n "${PROXY_SOURCE:-}" ] && check_path=$PROXY_SOURCE; set -- "$@" --enable-proxy --proxy-config "$check_path" ;;
                web) check_name="Web"; [ -n "${WEB_SOURCE:-}" ] && check_path=$WEB_SOURCE; set -- "$@" --enable-web --web-config "$check_path" ;;
            esac
            if [ ! -f "$check_path" ]; then
                if [ "$check_path" = "$MODULE_CONFIG" ]; then
                    warn "$check_name 配置文件不存在：$check_path；请基于 $ETC_DIR/$check_module/$MODULE_EXAMPLE 创建正式配置"
                else
                    warn "$check_name 配置文件不存在：$check_path"
                fi
                check_missing=1
            fi
        fi
    done
    [ "$check_missing" -eq 0 ] || return 1
    "$@"
}

configs_valid() {
    TUNNEL_SOURCE="" PROXY_SOURCE="" WEB_SOURCE="" check_configs_with "$BIN" "$1"
}

write_service_environment() {
    environment_modules=$1
    service_args=""
    has_module "$environment_modules" tunnel && service_args="$service_args --enable-tunnel --tunnel-config /etc/gaterust/tunnel/server.toml"
    has_module "$environment_modules" proxy && service_args="$service_args --enable-proxy --proxy-config /etc/gaterust/proxy/proxy.toml"
    has_module "$environment_modules" web && service_args="$service_args --enable-web --web-config /etc/gaterust/web/web.toml"
    service_args=${service_args# }
    printf 'GATERUST_ARGS=%s\n' "$service_args" > "$TEMP_DIR/service.env"
}

install_service_environment() {
    atomic_install "$TEMP_DIR/service.env" "$ENV_FILE" 0644 root root
}

write_service_files() {
    installed_modules=$1
    enabled_modules=$2
    write_service_environment "$enabled_modules"
    case "$SERVICE_MANAGER" in
        systemd)
            if has_module "$installed_modules" proxy; then
                awk '/@PROXY_CAPABILITIES@/ { print "AmbientCapabilities=CAP_NET_BIND_SERVICE"; print "CapabilityBoundingSet=CAP_NET_BIND_SERVICE"; next } { print }' "$package/gaterust.service" > "$TEMP_DIR/service-file"
            else
                sed '/@PROXY_CAPABILITIES@/d' "$package/gaterust.service" > "$TEMP_DIR/service-file"
            fi
            ;;
        openrc)
            if has_module "$installed_modules" proxy; then
                sed 's/@PROXY_CAPABILITIES@/capabilities="^cap_net_bind_service"/' "$package/gaterust.openrc" > "$TEMP_DIR/service-file"
            else
                sed '/@PROXY_CAPABILITIES@/d' "$package/gaterust.openrc" > "$TEMP_DIR/service-file"
            fi
            ;;
    esac
}

atomic_install() {
    install_source=$1
    install_target=$2
    install_mode=$3
    install_owner=$4
    install_group=$5
    install_dir=$(dirname "$install_target")
    mkdir -p "$install_dir"
    install -m "$install_mode" -o "$install_owner" -g "$install_group" "$install_source" "$install_target.new"
    mv -f "$install_target.new" "$install_target"
}

create_account() {
    if ! getent group gaterust >/dev/null 2>&1; then
        if command -v groupadd >/dev/null 2>&1; then
            groupadd --system gaterust
        elif command -v addgroup >/dev/null 2>&1; then
            addgroup -S gaterust
        else
            die "未找到系统组创建命令"
        fi
    fi
    if ! id gaterust >/dev/null 2>&1; then
        if command -v nologin >/dev/null 2>&1; then
            account_shell=$(command -v nologin)
        elif [ -x /bin/false ]; then
            account_shell=/bin/false
        else
            die "未找到不可登录 shell"
        fi
        if command -v useradd >/dev/null 2>&1; then
            useradd --system --gid gaterust --home-dir /var/lib/gaterust --no-create-home --shell "$account_shell" gaterust
        elif command -v adduser >/dev/null 2>&1; then
            adduser -S -D -H -h /var/lib/gaterust -s "$account_shell" -G gaterust gaterust
        else
            die "未找到系统用户创建命令"
        fi
    fi
    mkdir -p "$ETC_DIR" "$DATA_DIR" "$LIB_DIR"
    chown root:gaterust "$ETC_DIR"
    chmod 0750 "$ETC_DIR"
    chown root:root "$DATA_DIR" "$LIB_DIR"
    chmod 0755 "$DATA_DIR" "$LIB_DIR"
}

delete_account() {
    if id gaterust >/dev/null 2>&1; then
        if command -v userdel >/dev/null 2>&1; then
            userdel gaterust
        elif command -v deluser >/dev/null 2>&1; then
            deluser gaterust
        else
            die "未找到系统用户删除命令"
        fi
    fi
    if getent group gaterust >/dev/null 2>&1; then
        if command -v groupdel >/dev/null 2>&1; then
            groupdel gaterust
        elif command -v delgroup >/dev/null 2>&1; then
            delgroup gaterust
        else
            die "未找到系统组删除命令"
        fi
    fi
}

prepare_service_runtime() {
    [ "$SERVICE_MANAGER" = openrc ] || return 0
    mkdir -p "$LOG_DIR"
    chown root:gaterust "$LOG_DIR"
    chmod 0750 "$LOG_DIR"
    if [ ! -e "$LOG_FILE" ]; then
        : > "$LOG_FILE"
    fi
    chown root:gaterust "$LOG_FILE"
    chmod 0660 "$LOG_FILE"
}

save_backup() {
    backup_path=$1
    backup_name=$2
    if [ -e "$backup_path" ]; then
        cp -p "$backup_path" "$TEMP_DIR/backup/$backup_name"
    else
        : > "$TEMP_DIR/backup/$backup_name.absent"
    fi
}

restore_backup() {
    restore_path=$1
    restore_name=$2
    if [ -f "$TEMP_DIR/backup/$restore_name.absent" ]; then
        rm -f "$restore_path"
    else
        mkdir -p "$(dirname "$restore_path")"
        cp -p "$TEMP_DIR/backup/$restore_name" "$restore_path"
    fi
}

rollback_install() {
    TRANSACTION=0
    warn "启动失败，正在恢复原版本"
    service_stop >/dev/null 2>&1 || true
    [ "${OLD_ENABLED:-0}" -eq 1 ] || service_disable >/dev/null 2>&1 || true
    cleanup_generated_files
    restore_backup "$BIN" binary
    restore_backup "$CTL" control
    restore_backup "$SERVICE_FILE" service
    restore_backup "$ENV_FILE" environment
    restore_backup "$STATE_FILE" state
    if [ "${WEB_REPLACED:-0}" -eq 1 ]; then
        rm -rf "$LIB_DIR/web"
        if [ -d "$TEMP_DIR/web.old" ]; then
            mv "$TEMP_DIR/web.old" "$LIB_DIR/web"
        fi
    fi
    service_reload_manager || true
    if [ "${OLD_ENABLED:-0}" -eq 1 ]; then
        service_enable >/dev/null 2>&1 || true
    else
        service_disable >/dev/null 2>&1 || true
    fi
    [ "${OLD_ACTIVE:-0}" -eq 1 ] && service_start >/dev/null 2>&1 || true
}

install_module_files() {
    for install_module in tunnel proxy web; do
        has_module "$NEW_MODULES" "$install_module" || continue
        mkdir -p "$ETC_DIR/$install_module" "$DATA_DIR/$install_module"
        chown root:gaterust "$ETC_DIR/$install_module"
        config_dir_mode=0750
        if has_module "$NEW_MODULES" web; then
            case "$install_module" in tunnel|proxy) config_dir_mode=0770 ;; esac
        fi
        chmod "$config_dir_mode" "$ETC_DIR/$install_module"
        chown gaterust:gaterust "$DATA_DIR/$install_module"
        chmod 0750 "$DATA_DIR/$install_module"
        if [ "$install_module" = tunnel ] && [ -n "${GENERATED_TUNNEL_CERTIFICATE:-}" ]; then
            [ ! -e "$TUNNEL_CERTIFICATE" ] || die "QUIC 证书已存在：$TUNNEL_CERTIFICATE"
            [ ! -e "$TUNNEL_PRIVATE_KEY" ] || die "QUIC 私钥已存在：$TUNNEL_PRIVATE_KEY"
            GENERATED_TUNNEL_FILES_INSTALLED=1
            atomic_install "$GENERATED_TUNNEL_CERTIFICATE" "$TUNNEL_CERTIFICATE" 0640 root gaterust
            atomic_install "$GENERATED_TUNNEL_PRIVATE_KEY" "$TUNNEL_PRIVATE_KEY" 0640 root gaterust
        fi
        module_config "$install_module"
        eval_source=""
        case "$install_module" in tunnel) eval_source=${TUNNEL_INSTALL_SOURCE:-${TUNNEL_SOURCE:-}} ;; proxy) eval_source=${PROXY_SOURCE:-} ;; web) eval_source=${WEB_SOURCE:-} ;; esac
        if [ -n "$eval_source" ] && [ ! -f "$MODULE_CONFIG" ]; then
            atomic_install "$eval_source" "$MODULE_CONFIG" 0640 root gaterust
            if [ "$install_module" = proxy ] && [ -n "${GENERATED_PROXY_CONFIG:-}" ] && [ "$eval_source" = "$GENERATED_PROXY_CONFIG" ]; then
                GENERATED_PROXY_CONFIG_INSTALLED=1
            fi
        elif [ ! -f "$MODULE_CONFIG" ] && [ ! -f "$ETC_DIR/$install_module/$MODULE_EXAMPLE" ]; then
            atomic_install "$package/config/$MODULE_EXAMPLE" "$ETC_DIR/$install_module/$MODULE_EXAMPLE" 0640 root gaterust
        fi
        if [ -f "$MODULE_CONFIG" ]; then
            chown root:gaterust "$MODULE_CONFIG"
            chmod 0640 "$MODULE_CONFIG"
        fi
        if [ -f "$ETC_DIR/$install_module/$MODULE_EXAMPLE" ]; then
            chown root:gaterust "$ETC_DIR/$install_module/$MODULE_EXAMPLE"
            chmod 0640 "$ETC_DIR/$install_module/$MODULE_EXAMPLE"
        fi
    done
}

perform_install() {
    OLD_ACTIVE=0 OLD_ENABLED=0
    service_is_active && OLD_ACTIVE=1 || true
    service_is_enabled && OLD_ENABLED=1 || true
    mkdir -p "$TEMP_DIR/backup"
    save_backup "$BIN" binary
    save_backup "$CTL" control
    save_backup "$SERVICE_FILE" service
    save_backup "$ENV_FILE" environment
    save_backup "$STATE_FILE" state
    write_service_files "$NEW_MODULES" "$RUN_MODULES"
    create_account
    install_module_files
    WEB_REPLACED=0
    TRANSACTION=1
    if [ "$OLD_ACTIVE" -eq 1 ]; then
        service_stop
    fi
    atomic_install "$package/gaterust-server" "$BIN" 0755 root root
    atomic_install "$TEMP_DIR/gaterust.sh" "$CTL" 0755 root root
    atomic_install "$TEMP_DIR/service-file" "$SERVICE_FILE" "$SERVICE_FILE_MODE" root root
    install_service_environment
    prepare_service_runtime
    printf 'VERSION=%s\nARCH=%s\nMODULES=%s\n' "$SCRIPT_VERSION" "$ARCH" "$NEW_MODULES" > "$TEMP_DIR/install-state"
    atomic_install "$TEMP_DIR/install-state" "$STATE_FILE" 0644 root root
    if has_module "$NEW_MODULES" web; then
        WEB_REPLACED=1
        rm -rf "$TEMP_DIR/web.new"
        cp -a "$package/web" "$TEMP_DIR/web.new"
        chown -R root:root "$TEMP_DIR/web.new"
        if [ -d "$LIB_DIR/web" ]; then mv "$LIB_DIR/web" "$TEMP_DIR/web.old"; fi
        mkdir -p "$LIB_DIR"
        mv "$TEMP_DIR/web.new" "$LIB_DIR/web"
    fi
    service_reload_manager

    FINAL_VALID=0
    if [ -n "$RUN_MODULES" ]; then
        configs_valid "$RUN_MODULES" >/dev/null 2>&1 && FINAL_VALID=1 || true
    fi
    if [ "$FINAL_VALID" -eq 0 ]; then
        START_MODE=stop
        warn "已安装示例或无效配置，服务保持停止且不开机启动"
    fi
    case "$START_MODE" in
        enable)
            service_enable
            service_start || { rollback_install; die "服务启动失败"; }
            ;;
        start)
            service_disable >/dev/null 2>&1 || true
            service_start || { rollback_install; die "服务启动失败"; }
            ;;
        preserve)
            if [ "$OLD_ENABLED" -eq 1 ]; then
                service_enable >/dev/null
            else
                service_disable >/dev/null 2>&1 || true
            fi
            if [ "$OLD_ACTIVE" -eq 1 ] && ! service_start; then
                rollback_install
                die "升级后服务启动失败"
            fi
            ;;
        stop)
            service_is_active && service_stop >/dev/null 2>&1 || true
            service_disable >/dev/null 2>&1 || true
            ;;
    esac
    GENERATED_TUNNEL_FILES_INSTALLED=0
    GENERATED_PROXY_CONFIG_INSTALLED=0
    TRANSACTION=0
    rm -f "$LEGACY_CTL"
    rm -rf "$TEMP_DIR/web.old"
    say "GateRust $SCRIPT_VERSION 安装完成"
    say "已安装模块：$(display_modules "$NEW_MODULES")"
    say "服务配置模块：$(display_modules "$RUN_MODULES")"
    if [ -n "${GENERATED_WEB_CONFIG:-}" ]; then
        say "Web 管理端口：TCP 8080（监听所有 IPv4 地址）"
        say "Web 管理用户：admin"
        if [ -n "${GENERATED_WEB_PASSWORD:-}" ]; then
            say "Web 初始密码：$GENERATED_WEB_PASSWORD"
            warn "Web 初始密码只显示这一次，请立即妥善保存并限制主机登录权限"
        else
            say "Web 管理密码：已使用安装时设置的密码"
        fi
    fi
    if [ -n "${GENERATED_TUNNEL_CERTIFICATE:-}" ]; then
        say "QUIC TLS 已自动初始化（自签名证书，服务器名称：gaterust.local）"
        say "QUIC 服务端证书：/etc/gaterust/tunnel/server.pem"
        warn "客户端需要信任该证书，并将 TLS 服务器名称设置为 gaterust.local"
    fi
    if [ -n "${GENERATED_PROXY_CONFIG:-}" ]; then
        say "Proxy 已自动初始化（HTTP 0.0.0.0:80，HTTPS 0.0.0.0:443）"
        say "自动 SSL 尚无托管证书，请通过 Web 或 /etc/gaterust/proxy/proxy.toml 添加真实域名和证书配置"
    fi
}

tty_read() {
    [ -r /dev/tty ] || die "交互模式需要可用的 /dev/tty，请改用命令行参数"
    printf '%s' "$1" > /dev/tty
    IFS= read -r REPLY < /dev/tty || die "读取交互输入失败"
}

tty_read_secret() {
    [ -r /dev/tty ] || die "交互模式需要可用的 /dev/tty，请改用命令行参数"
    require_command stty
    printf '%s' "$1" > /dev/tty
    TTY_STATE=$(stty -g < /dev/tty) || die "读取终端状态失败"
    if ! stty -echo < /dev/tty; then
        restore_tty
        die "关闭终端回显失败"
    fi
    secret_read_status=0
    IFS= read -r REPLY < /dev/tty || secret_read_status=$?
    restore_tty
    printf '\n' > /dev/tty
    [ "$secret_read_status" -eq 0 ] || die "读取交互输入失败"
}

generate_web_config_interactively() {
    tty_read_secret "请输入 Web 管理员 admin 的密码（留空则自动生成）："
    web_password_input=$REPLY
    if [ -n "$web_password_input" ]; then
        tty_read_secret "请再次输入 Web 管理员密码："
        [ "$REPLY" = "$web_password_input" ] || die "两次输入的 Web 管理员密码不一致"
    fi
    generate_web_config "$web_password_input"
    web_password_input=""
}

interactive_modules() {
    say "请选择安装模块："
    say "  1. QUIC 内网穿透"
    say "  2. 反向代理 + 自动 SSL"
    say "  3. Web 管理界面"
    say "  4. 全部安装"
    say "  0. 返回"
    tty_read "请输入模块编号，多个用逗号分隔 [默认 4]："
    selection=${REPLY:-4}
    [ "$selection" = 0 ] && return 1
    case ",$selection," in *,4,*) [ "$selection" = 4 ] || die "4 不能与其他编号同时使用"; NORMALIZED="tunnel,proxy,web"; return 0 ;; esac
    number_modules=""
    old_ifs=$IFS IFS=,; set -- $selection; IFS=$old_ifs
    for number in "$@"; do
        number=$(printf '%s' "$number" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        case "$number" in 1) name=tunnel ;; 2) name=proxy ;; 3) name=web ;; 0) die "0 不能与其他编号同时使用" ;; *) die "无效模块编号：$number" ;; esac
        has_module "$number_modules" "$name" || number_modules="${number_modules:+$number_modules,}$name"
    done
    [ -n "$number_modules" ] || die "至少选择一个模块"
    NORMALIZED=$number_modules
}

choose_configs() {
    TUNNEL_SOURCE="" TUNNEL_INSTALL_SOURCE="" PROXY_SOURCE="" WEB_SOURCE="" EXAMPLE_SELECTED=0
    GENERATED_TUNNEL_CERTIFICATE="" GENERATED_TUNNEL_PRIVATE_KEY="" GENERATED_PROXY_CONFIG=""
    GENERATED_WEB_CONFIG="" GENERATED_WEB_PASSWORD=""
    GENERATED_TUNNEL_FILES_INSTALLED=0 GENERATED_PROXY_CONFIG_INSTALLED=0
    for choose_module in tunnel proxy web; do
        has_module "$NEW_MODULES" "$choose_module" || continue
        module_config "$choose_module"
        [ -f "$MODULE_CONFIG" ] && continue
        if [ "$INTERACTIVE" -eq 1 ]; then
            say ""
            case "$choose_module" in
                tunnel)
                    say "QUIC 配置："
                    say "  1. 自动生成证书和私钥"
                    say "  2. 导入已有配置"
                    say "  3. 仅安装示例配置"
                    tty_read "请选择 [默认 1]："
                    case "${REPLY:-1}" in
                        1) generate_tunnel_config; choose_source=$GENERATED_TUNNEL_CHECK_CONFIG ;;
                        2) tty_read "请输入配置文件路径："; [ -f "$REPLY" ] || die "配置文件不存在：$REPLY"; choose_source=$REPLY ;;
                        3) choose_source=""; EXAMPLE_SELECTED=1 ;;
                        *) die "无效选择" ;;
                    esac
                    ;;
                proxy)
                    say "Proxy 配置："
                    say "  1. 自动生成最小配置"
                    say "  2. 导入已有配置"
                    say "  3. 仅安装示例配置"
                    tty_read "请选择 [默认 1]："
                    case "${REPLY:-1}" in
                        1) generate_proxy_config; choose_source=$GENERATED_PROXY_CONFIG ;;
                        2) tty_read "请输入配置文件路径："; [ -f "$REPLY" ] || die "配置文件不存在：$REPLY"; choose_source=$REPLY ;;
                        3) choose_source=""; EXAMPLE_SELECTED=1 ;;
                        *) die "无效选择" ;;
                    esac
                    ;;
                web)
                    say "Web 配置："
                    say "  1. 自动安全初始化"
                    say "  2. 导入已有配置"
                    say "  3. 仅安装示例配置"
                    tty_read "请选择 [默认 1]："
                    case "${REPLY:-1}" in
                        1) generate_web_config_interactively; choose_source=$GENERATED_WEB_CONFIG ;;
                        2) tty_read "请输入配置文件路径："; [ -f "$REPLY" ] || die "配置文件不存在：$REPLY"; choose_source=$REPLY ;;
                        3) choose_source=""; EXAMPLE_SELECTED=1 ;;
                        *) die "无效选择" ;;
                    esac
                    ;;
            esac
        else
            case "$choose_module" in tunnel) choose_source=${TUNNEL_SOURCE_ARG:-} ;; proxy) choose_source=${PROXY_SOURCE_ARG:-} ;; web) choose_source=${WEB_SOURCE_ARG:-} ;; esac
            if [ "$choose_module" = tunnel ] && [ "$INIT_TUNNEL" -eq 1 ]; then
                generate_tunnel_config
                choose_source=$GENERATED_TUNNEL_CHECK_CONFIG
            elif [ "$choose_module" = proxy ] && [ "$INIT_PROXY" -eq 1 ]; then
                generate_proxy_config
                choose_source=$GENERATED_PROXY_CONFIG
            elif [ -z "$choose_source" ] && [ "$choose_module" = web ]; then
                generate_web_config
                choose_source=$GENERATED_WEB_CONFIG
            elif [ -z "$choose_source" ]; then
                EXAMPLE_SELECTED=1
            fi
        fi
        case "$choose_module" in tunnel) TUNNEL_SOURCE=$choose_source ;; proxy) PROXY_SOURCE=$choose_source ;; web) WEB_SOURCE=$choose_source ;; esac
    done
}

random_hex() {
    random_bytes=$1
    command -v od >/dev/null 2>&1 || die "自动初始化 Web 需要 od"
    command -v tr >/dev/null 2>&1 || die "自动初始化 Web 需要 tr"
    od -An -N "$random_bytes" -tx1 /dev/urandom | tr -d ' \n'
}

write_tunnel_config() {
    tunnel_config_path=$1
    tunnel_certificate_path=$2
    tunnel_private_key_path=$3
    {
        printf '%s\n' '[quic]'
        printf '%s\n' 'bind = "0.0.0.0:2333"'
        printf 'certificate = "%s"\n' "$tunnel_certificate_path"
        printf 'private_key = "%s"\n' "$tunnel_private_key_path"
    } > "$tunnel_config_path"
}

require_tunnel_init_targets_available() {
    [ ! -e "$TUNNEL_CONFIG" ] || die "QUIC 正式配置已存在，不能自动初始化"
    [ ! -e "$TUNNEL_CERTIFICATE" ] || die "QUIC 证书已存在，请选择导入已有配置"
    [ ! -e "$TUNNEL_PRIVATE_KEY" ] || die "QUIC 私钥已存在，请选择导入已有配置"
}

generate_tunnel_config() {
    require_tunnel_init_targets_available
    ensure_openssl
    GENERATED_TUNNEL_CERTIFICATE="$TEMP_DIR/server.pem"
    GENERATED_TUNNEL_PRIVATE_KEY="$TEMP_DIR/server-key.pem"
    GENERATED_TUNNEL_CONFIG="$TEMP_DIR/server.toml"
    GENERATED_TUNNEL_CHECK_CONFIG="$TEMP_DIR/server-check.toml"
    (
        umask 077
        openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 3650 \
            -subj '/CN=gaterust.local' \
            -addext 'subjectAltName=DNS:gaterust.local' \
            -addext 'basicConstraints=critical,CA:FALSE' \
            -addext 'keyUsage=critical,digitalSignature,keyEncipherment' \
            -addext 'extendedKeyUsage=serverAuth' \
            -keyout "$GENERATED_TUNNEL_PRIVATE_KEY" \
            -out "$GENERATED_TUNNEL_CERTIFICATE" >/dev/null 2>&1 || exit 1
        write_tunnel_config "$GENERATED_TUNNEL_CONFIG" \
            "$TUNNEL_CERTIFICATE" "$TUNNEL_PRIVATE_KEY"
        write_tunnel_config "$GENERATED_TUNNEL_CHECK_CONFIG" \
            "$GENERATED_TUNNEL_CERTIFICATE" "$GENERATED_TUNNEL_PRIVATE_KEY"
    ) || die "生成 QUIC 证书和私钥失败"
    TUNNEL_INSTALL_SOURCE=$GENERATED_TUNNEL_CONFIG
}

require_proxy_init_target_available() {
    [ ! -e "$PROXY_CONFIG" ] || die "Proxy 正式配置已存在，不能自动初始化"
}

generate_proxy_config() {
    require_proxy_init_target_available
    GENERATED_PROXY_CONFIG="$TEMP_DIR/proxy.toml"
    (
        umask 077
        {
            printf '%s\n' '[proxy]'
            printf '%s\n' 'http_bind = "0.0.0.0:80"'
            printf '%s\n' 'https_bind = "0.0.0.0:443"'
            printf '%s\n' 'cache_dir = "/var/lib/gaterust/proxy/acme"'
            printf '%s\n' 'max_connections = 2048'
        } > "$GENERATED_PROXY_CONFIG"
    )
}

generate_web_config() {
    generated_web_password=${1:-}
    if [ -z "$generated_web_password" ]; then
        GENERATED_WEB_PASSWORD=$(random_hex 16)
        generated_web_password=$GENERATED_WEB_PASSWORD
    else
        GENERATED_WEB_PASSWORD=""
    fi
    generated_jwt_secret=$(random_hex 32)
    generated_password_hash=$(printf '%s' "$generated_web_password" | "$package/gaterust-server" hash-password) || die "生成 Web 管理员密码哈希失败"
    generated_web_password=""
    GENERATED_WEB_CONFIG="$TEMP_DIR/web.toml"
    (
        umask 077
        {
            printf '%s\n' '[web]'
            printf '%s\n' 'bind = "0.0.0.0:8080"'
            printf '%s\n' 'static_dir = "/usr/local/lib/gaterust/web"'
            printf '%s\n' 'admin_username = "admin"'
            printf 'admin_password_hash = "%s"\n' "$generated_password_hash"
            printf 'jwt_secret = "%s"\n' "$generated_jwt_secret"
            printf '%s\n' 'token_ttl_seconds = 3600'
            printf '%s\n' 'allowed_origins = []'
        } > "$GENERATED_WEB_CONFIG"
    )
}

install_command() {
    require_root
    require_platform
    acquire_lock
    had_state=0
    if read_state; then had_state=1; existing_modules=$STATE_MODULES; else existing_modules=""; fi
    if [ -z "$REQUEST_MODULES" ]; then
        [ "$had_state" -eq 1 ] && REQUEST_MODULES=$existing_modules || die "install 需要 --modules"
    fi
    normalize_modules "$REQUEST_MODULES"
    merge_modules "$existing_modules" "$NORMALIZED"
    NEW_MODULES=$NORMALIZED
    if [ "$INIT_TUNNEL" -eq 1 ]; then
        has_module "$NEW_MODULES" tunnel || die "--init-tunnel 需要安装 QUIC 模块"
        [ -z "$TUNNEL_SOURCE_ARG" ] || die "--init-tunnel 不能与 --tunnel-config 同时使用"
        require_tunnel_init_targets_available
    fi
    if [ "$INIT_PROXY" -eq 1 ]; then
        has_module "$NEW_MODULES" proxy || die "--init-proxy 需要安装 Proxy 模块"
        [ -z "$PROXY_SOURCE_ARG" ] || die "--init-proxy 不能与 --proxy-config 同时使用"
        require_proxy_init_target_available
    fi
    if [ "$FORCE_INSTALL" -eq 0 ] && [ "$INIT_TUNNEL" -eq 0 ] && [ "$INIT_PROXY" -eq 0 ] && [ "$had_state" -eq 1 ] && [ "$STATE_VERSION" = "$SCRIPT_VERSION" ] && [ "$NEW_MODULES" = "$existing_modules" ]; then
        say "GateRust $SCRIPT_VERSION 和所选模块已安装，无需更新"
        release_lock
        return
    fi
    prepare_release
    choose_configs
    valid_modules=""
    for validate_module in tunnel proxy web; do
        has_module "$NEW_MODULES" "$validate_module" || continue
        module_config "$validate_module"
        validate_source=""
        case "$validate_module" in tunnel) validate_source=${TUNNEL_SOURCE:-} ;; proxy) validate_source=${PROXY_SOURCE:-} ;; web) validate_source=${WEB_SOURCE:-} ;; esac
        if [ -n "$validate_source" ] || [ -f "$MODULE_CONFIG" ]; then
            valid_modules="${valid_modules:+$valid_modules,}$validate_module"
        fi
    done
    if [ -n "$valid_modules" ]; then
        check_configs_with "$package/gaterust-server" "$valid_modules" || die "配置校验失败"
    fi
    RUN_MODULES=$valid_modules
    if [ "$INTERACTIVE" -eq 1 ]; then
        if [ -z "$RUN_MODULES" ]; then
            START_MODE=stop
            say "没有可运行的正式配置，服务将保持停止且不开机启动。"
        else
            [ "$EXAMPLE_SELECTED" -eq 0 ] || say "未配置的模块本次不会启动，可在 Web 保存配置后执行 gaterust restart 启用。"
            say "启动方式："
            say "  1. 立即启动并启用开机启动"
            say "  2. 立即启动"
            say "  3. 暂不启动"
            tty_read "请选择 [默认 1]："
            case "${REPLY:-1}" in 1) START_MODE=enable ;; 2) START_MODE=start ;; 3) START_MODE=stop ;; *) die "无效选择" ;; esac
        fi
        say "安装摘要：版本 $SCRIPT_VERSION，架构 $ARCH，模块 $NEW_MODULES"
        tty_read "输入 y 确认安装："
        [ "$REPLY" = y ] || die "已取消安装"
    fi
    [ "$had_state" -eq 1 ] && [ "$START_MODE" = default ] && START_MODE=preserve
    [ "$START_MODE" = default ] && START_MODE=stop
    [ -n "$RUN_MODULES" ] || START_MODE=stop
    perform_install
    release_lock
}

collect_configured_modules() {
    configured_from=$1
    RUN_MODULES=""
    for configured_module in tunnel proxy web; do
        has_module "$configured_from" "$configured_module" || continue
        module_config "$configured_module"
        if [ -f "$MODULE_CONFIG" ]; then
            RUN_MODULES="${RUN_MODULES:+$RUN_MODULES,}$configured_module"
        else
            case "$configured_module" in tunnel) configured_name="QUIC" ;; proxy) configured_name="Proxy" ;; web) configured_name="Web" ;; esac
            warn "$configured_name 尚无正式配置，本次不启用；示例位于 $ETC_DIR/$configured_module/$MODULE_EXAMPLE"
        fi
    done
}

prepare_service_config() {
    read_state || die "GateRust 尚未安装"
    collect_configured_modules "$STATE_MODULES"
    [ -n "$RUN_MODULES" ] || die "没有可运行的模块，请先创建至少一个正式配置"
    configs_valid "$RUN_MODULES" || die "配置校验失败，服务未操作"
    write_service_environment "$RUN_MODULES"
    install_service_environment
}

service_command() {
    service_action=$1
    case "$service_action" in
        start|stop|restart|enable|disable|logs) require_root ;;
    esac
    require_platform
    case "$service_action" in
        start|restart|enable)
            acquire_lock
            prepare_service_config
            ;;
    esac
    case "$service_action" in
        stop|disable|logs) read_state >/dev/null || die "GateRust 尚未安装" ;;
    esac
    case "$service_action" in
        start) service_start ;;
        stop) service_stop ;;
        restart) service_restart ;;
        enable) service_enable ;;
        disable) service_disable ;;
        logs) service_logs ;;
    esac
    case "$service_action" in
        start|restart|enable)
            say "运行模块：$(display_modules "$RUN_MODULES")"
            release_lock
            ;;
    esac
}

read_service_modules() {
    SERVICE_MODULES=""
    service_environment=""
    [ -r "$ENV_FILE" ] || return 0
    IFS= read -r service_environment < "$ENV_FILE" || return 0
    case "$service_environment" in GATERUST_ARGS=*) service_arguments=${service_environment#GATERUST_ARGS=} ;; *) return 0 ;; esac
    case " $service_arguments " in *" --enable-tunnel "*) SERVICE_MODULES=tunnel ;; esac
    case " $service_arguments " in *" --enable-proxy "*) SERVICE_MODULES="${SERVICE_MODULES:+$SERVICE_MODULES,}proxy" ;; esac
    case " $service_arguments " in *" --enable-web "*) SERVICE_MODULES="${SERVICE_MODULES:+$SERVICE_MODULES,}web" ;; esac
}

status_command() {
    require_platform
    read_state || die "GateRust 尚未安装"
    read_service_modules
    status_active="已停止" status_enabled="未启用" status_pid="-" status_uptime="-"
    service_is_enabled && status_enabled="已启用" || true
    if service_is_active; then
        detected_pid=$(service_main_pid 2>/dev/null || true)
        case "$detected_pid" in
            ''|0|*[!0-9]*) status_active="启动异常" ;;
            *) status_active="运行中"; status_pid=$detected_pid ;;
        esac
    fi
    if [ "$status_active" = "运行中" ]; then
        status_seconds=$(service_uptime_seconds "$status_pid" 2>/dev/null || true)
        if [ -n "$status_seconds" ]; then
            status_days=$((status_seconds / 86400))
            status_hours=$(((status_seconds % 86400) / 3600))
            status_minutes=$(((status_seconds % 3600) / 60))
            status_uptime="${status_days}天 ${status_hours}小时 ${status_minutes}分钟"
        fi
    fi
    say "版本：$STATE_VERSION"
    say "架构：$STATE_ARCH"
    say "已安装模块：$(display_modules "$STATE_MODULES")"
    say "运行模块：$(display_modules "$SERVICE_MODULES")"
    say "配置目录：/etc/gaterust"
    say "服务：$status_active"
    say "开机启动：$status_enabled"
    say "PID：$status_pid"
    say "运行时间：$status_uptime"
}

delete_module_files() {
    delete_module=$1
    [ "$KEEP_CONFIG" -eq 1 ] || rm -rf "$ETC_DIR/$delete_module"
    rm -rf "$DATA_DIR/$delete_module"
    [ "$delete_module" = web ] && rm -rf "$LIB_DIR/web"
}

full_uninstall() {
    if service_is_active; then
        service_stop
    fi
    service_disable >/dev/null 2>&1 || true
    rm -f "$SYSTEMD_SERVICE_FILE" "$OPENRC_SERVICE_FILE"
    service_reload_manager
    service_reset_failed >/dev/null 2>&1 || true
    rm -f "$BIN"
    if [ "$KEEP_CONFIG" -eq 1 ]; then
        chown -R root:root "$ETC_DIR"
    else
        rm -rf "$ETC_DIR"
    fi
    rm -rf "$DATA_DIR" "$LIB_DIR" "$LOG_DIR"
    delete_account
    rm -f "$CTL" "$LEGACY_CTL"
    say "GateRust 已完整卸载"
}

confirm_uninstall() {
    [ "$ASSUME_YES" -eq 1 ] && return
    tty_read "以上内容将被删除，输入 y 确认："
    [ "$REPLY" = y ] || die "已取消卸载"
}

show_uninstall_files() {
    if [ "$UNINSTALL_ALL" -eq 1 ]; then
        say "将删除："
        say "  /usr/local/bin/gaterust-server"
        say "  /usr/local/sbin/gaterust"
        say "  ${SERVICE_FILE#$ROOT}"
        [ "$KEEP_CONFIG" -eq 1 ] || say "  /etc/gaterust/"
        say "  /var/lib/gaterust/"
        say "  /usr/local/lib/gaterust/"
        [ "$SERVICE_MANAGER" != openrc ] || say "  /var/log/gaterust/"
        say "  gaterust 系统用户和组"
        return
    fi
    say "将删除："
    for show_module in tunnel proxy web; do
        has_module "$REMOVE_MODULES" "$show_module" || continue
        [ "$KEEP_CONFIG" -eq 1 ] || say "  /etc/gaterust/$show_module/"
        say "  /var/lib/gaterust/$show_module/"
        [ "$show_module" = web ] && say "  /usr/local/lib/gaterust/web/"
    done
}

uninstall_command() {
    require_root
    require_platform
    acquire_lock
    read_state || die "GateRust 尚未安装"
    if [ "$UNINSTALL_ALL" -eq 1 ]; then
        show_uninstall_files
        confirm_uninstall
        full_uninstall
        release_lock
        return
    fi
    [ -n "$REQUEST_MODULES" ] || die "uninstall 需要 --modules 或 --all"
    normalize_modules "$REQUEST_MODULES"
    REMOVE_MODULES=$NORMALIZED
    old_ifs=$IFS IFS=,; set -- $REMOVE_MODULES; IFS=$old_ifs
    for remove_module in "$@"; do has_module "$STATE_MODULES" "$remove_module" || die "模块未安装：$remove_module"; done
    remove_modules "$STATE_MODULES" "$REMOVE_MODULES"
    remaining=$NORMALIZED
    if [ -n "$remaining" ]; then
        prepare_release
        collect_configured_modules "$remaining"
        write_service_files "$remaining" "$RUN_MODULES"
        printf 'VERSION=%s\nARCH=%s\nMODULES=%s\n' "$STATE_VERSION" "$STATE_ARCH" "$remaining" > "$TEMP_DIR/install-state"
    fi
    say "将卸载模块：$(display_modules "$REMOVE_MODULES")"
    show_uninstall_files
    [ "$KEEP_CONFIG" -eq 1 ] && say "配置目录将保留。"
    confirm_uninstall
    was_active=0 was_enabled=0
    service_is_active && was_active=1 || true
    service_is_enabled && was_enabled=1 || true
    if [ "$was_active" -eq 1 ]; then
        service_stop
    fi
    for remove_module in "$@"; do delete_module_files "$remove_module"; done
    if [ -z "$remaining" ]; then full_uninstall; release_lock; return; fi
    if ! has_module "$remaining" web; then
        for protected_module in tunnel proxy; do
            has_module "$remaining" "$protected_module" && chmod 0750 "$ETC_DIR/$protected_module"
        done
    fi
    NEW_MODULES=$remaining
    atomic_install "$TEMP_DIR/service-file" "$SERVICE_FILE" "$SERVICE_FILE_MODE" root root
    install_service_environment
    atomic_install "$TEMP_DIR/install-state" "$STATE_FILE" 0644 root root
    service_reload_manager
    [ "$was_enabled" -eq 1 ] && service_enable >/dev/null || true
    if [ "$was_active" -eq 1 ]; then
        [ -n "$RUN_MODULES" ] && configs_valid "$RUN_MODULES" && service_start || die "剩余模块没有有效配置，服务保持停止"
    fi
    say "已卸载模块：$REMOVE_MODULES；剩余模块：$remaining"
    release_lock
}

interactive_service_menu() {
    while :; do
        say "  1. 启动服务"
        say "  2. 停止服务"
        say "  3. 重启服务"
        say "  4. 启用开机启动"
        say "  5. 关闭开机启动"
        say "  6. 查看实时日志"
        say "  0. 返回"
        tty_read "请选择："
        case "$REPLY" in
            1) interactive_service_command start ;;
            2) interactive_service_command stop ;;
            3) interactive_service_command restart ;;
            4) interactive_service_command enable ;;
            5) interactive_service_command disable ;;
            6) interactive_service_command logs ;;
            0) return ;;
            *) warn "无效选择" ;;
        esac
    done
}

interactive_service_command() {
    if [ "$(id -u)" -eq 0 ]; then
        service_command "$1"
    else
        run_installed_as_root "$1"
    fi
}

interactive_uninstall() {
    say "请选择卸载内容："
    say "  1. QUIC 内网穿透"
    say "  2. 反向代理 + 自动 SSL"
    say "  3. Web 管理界面"
    say "  4. 完整卸载 GateRust"
    say "  0. 返回"
    tty_read "请输入模块编号，多个用逗号分隔："
    [ "$REPLY" = 0 ] && return
    if [ "$REPLY" = 4 ]; then UNINSTALL_ALL=1; else
        case ",$REPLY," in *,4,*) die "4 不能与其他编号同时使用" ;; esac
        selection=$REPLY
        number_modules=""
        old_ifs=$IFS IFS=,; set -- $selection; IFS=$old_ifs
        for number in "$@"; do number=$(printf '%s' "$number" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//'); case "$number" in 1) name=tunnel ;; 2) name=proxy ;; 3) name=web ;; *) die "无效模块编号：$number" ;; esac; has_module "$number_modules" "$name" || number_modules="${number_modules:+$number_modules,}$name"; done
        REQUEST_MODULES=$number_modules
    fi
    if [ "$(id -u)" -eq 0 ]; then
        uninstall_command
    elif [ "$UNINSTALL_ALL" -eq 1 ]; then
        run_installed_as_root uninstall --all
    else
        run_installed_as_root uninstall --modules "$REQUEST_MODULES"
    fi
}

interactive_install() {
    INTERACTIVE=1
    interactive_modules || return
    REQUEST_MODULES=$NORMALIZED
    install_command
}

interactive_install_as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        interactive_install
    else
        run_installed_as_root internal-interactive-install
    fi
}

interactive_main() {
    INTERACTIVE=1
    if ! read_state; then
        say "GateRust 安装管理程序"
        say ""
        interactive_install_as_root
        return
    fi
    while :; do
        say "GateRust 管理程序"
        say ""
        status_command
        say ""
        say "  1. 安装或更新模块"
        say "  2. 服务管理"
        say "  3. 查看安装信息和服务状态"
        say "  4. 卸载模块"
        say "  0. 退出"
        tty_read "请选择："
        case "$REPLY" in
            1) interactive_install_as_root ;;
            2) interactive_service_menu ;;
            3) status_command ;;
            4) interactive_uninstall; [ -f "$STATE_FILE" ] || return ;;
            0) return ;;
            *) warn "无效选择" ;;
        esac
    done
}

REQUEST_MODULES="" TUNNEL_SOURCE_ARG="" PROXY_SOURCE_ARG="" WEB_SOURCE_ARG=""
START_MODE=default INTERACTIVE=0 ASSUME_YES=0 KEEP_CONFIG=0 UNINSTALL_ALL=0 FORCE_INSTALL=0 INIT_TUNNEL=0 INIT_PROXY=0
[ "${GATERUST_LIBRARY_ONLY:-0}" -eq 1 ] && return 0
command_name=${1:-}
case "$command_name" in
    install|start|stop|restart|enable|disable|logs|uninstall)
        if [ "$(id -u)" -ne 0 ] && [ "$0" = "$CTL" ]; then
            run_installed_as_root "$@"
            exit $?
        fi
        ;;
esac
if [ -n "$command_name" ]; then shift; fi
while [ "$#" -gt 0 ]; do
    case "$1" in
        --modules) [ "$#" -ge 2 ] || die "--modules 缺少参数"; REQUEST_MODULES=$2; shift 2 ;;
        --init-tunnel) INIT_TUNNEL=1; shift ;;
        --init-proxy) INIT_PROXY=1; shift ;;
        --tunnel-config) [ "$#" -ge 2 ] || die "--tunnel-config 缺少参数"; TUNNEL_SOURCE_ARG=$2; shift 2 ;;
        --proxy-config) [ "$#" -ge 2 ] || die "--proxy-config 缺少参数"; PROXY_SOURCE_ARG=$2; shift 2 ;;
        --web-config) [ "$#" -ge 2 ] || die "--web-config 缺少参数"; WEB_SOURCE_ARG=$2; shift 2 ;;
        --start) START_MODE=start; shift ;;
        --enable) START_MODE=enable; shift ;;
        --force) FORCE_INSTALL=1; shift ;;
        --yes) ASSUME_YES=1; shift ;;
        --keep-config) KEEP_CONFIG=1; shift ;;
        --all) UNINSTALL_ALL=1; shift ;;
        *) die "未知参数：$1" ;;
    esac
done

case "$command_name" in
    install) install_command ;;
    start|stop|restart|enable|disable|logs) service_command "$command_name" ;;
    status) status_command ;;
    uninstall) uninstall_command ;;
    internal-interactive-install) interactive_install ;;
    '')
        if [ -f "$STATE_FILE" ] && [ "$(basename "$0")" != gaterust ]; then
            REQUEST_MODULES=$(awk -F= '$1 == "MODULES" { print $2 }' "$STATE_FILE")
            install_command
        else
            interactive_main
        fi
        ;;
    *) die "未知命令：$command_name" ;;
esac
