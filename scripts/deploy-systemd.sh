#!/bin/bash

# systemd 本机部署脚本（不使用 Docker）
#
# 做的事：
#   1. 构建后端 release 二进制 + 前端 dist
#   2. 同步到部署目录（默认 /home/hyx/opt/baidu-netdisk）
#   3. 备份现有配置后保留（配置与凭证不覆盖）
#   4. 重启 systemd **用户**单元 baidu-netdisk.service
#   5. 健康检查
#
# 用法：
#   scripts/deploy-systemd.sh                 # 构建 + 部署 + 重启
#   scripts/deploy-systemd.sh --skip-build    # 只同步已有产物（本地已构建过）
#   scripts/deploy-systemd.sh --no-restart    # 只部署不重启
#
# 单元文件在 ~/.config/systemd/user/baidu-netdisk.service，
# WorkingDirectory=部署目录，因此 app.toml 里的相对路径
# （config/ logs/ wal/ downloads/）都相对部署目录解析。

set -euo pipefail

# ----------------- 颜色 -----------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log()  { echo -e "${BLUE}[$(date '+%H:%M:%S')]${NC} $*"; }
ok()   { echo -e "${GREEN}✅ $*${NC}"; }
warn() { echo -e "${YELLOW}⚠️  $*${NC}"; }
err()  { echo -e "${RED}❌ $*${NC}"; exit 1; }

# ----------------- 路径 -----------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BACKEND_DIR="$PROJECT_ROOT/backend"
FRONTEND_DIR="$PROJECT_ROOT/frontend"

DEPLOY_DIR="${DEPLOY_DIR:-/home/hyx/opt/baidu-netdisk}"
BIN_NAME="baidu-netdisk-rust"
SERVICE_NAME="baidu-netdisk.service"
HEALTH_PORT="${HEALTH_PORT:-18888}"

SKIP_BUILD=false
DO_RESTART=true

for arg in "$@"; do
    case "$arg" in
        --skip-build)  SKIP_BUILD=true ;;
        --no-restart)  DO_RESTART=false ;;
        -h|--help)     sed -n '2,20p' "$0"; exit 0 ;;
        *)             err "未知参数: $arg" ;;
    esac
done

# ----------------- 构建 -----------------
if [ "$SKIP_BUILD" = false ]; then
    log "构建后端（release）…"
    (cd "$BACKEND_DIR" && cargo build --release)
    ok "后端构建完成"

    log "构建前端（vite build）…"
    (cd "$FRONTEND_DIR" && npm run build)
    ok "前端构建完成"
else
    warn "跳过构建（--skip-build）"
fi

# ----------------- 同步产物 -----------------
[ -f "$BACKEND_DIR/target/release/$BIN_NAME" ] || err "未找到后端二进制，请先构建"
[ -f "$FRONTEND_DIR/dist/index.html" ] || err "未找到前端产物，请先构建"

log "同步到部署目录 $DEPLOY_DIR"
mkdir -p "$DEPLOY_DIR"/{config,data,logs,wal,downloads,frontend/dist}

# 先停服务再覆盖二进制：运行中的可执行文件不能原地覆盖
if [ "$DO_RESTART" = true ]; then
    systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true
fi

# 二进制
install -m 0755 "$BACKEND_DIR/target/release/$BIN_NAME" "$DEPLOY_DIR/$BIN_NAME.new"
mv -f "$DEPLOY_DIR/$BIN_NAME.new" "$DEPLOY_DIR/$BIN_NAME"
ok "后端二进制已更新"

# 前端（先清空再同步，避免残留旧 chunk 被 index.html 引用错乱）
rm -rf "$DEPLOY_DIR/frontend/dist"
cp -r "$FRONTEND_DIR/dist" "$DEPLOY_DIR/frontend/dist"
ok "前端资源已更新"

# ----------------- 配置 -----------------
# 配置与凭证**不覆盖**：部署目录已有 config/app.toml 与 config/auth.json，
# 覆盖会丢掉认证模式、账号会话和 2FA 密钥。仅在配置缺失时从仓库复制。
if [ ! -f "$DEPLOY_DIR/config/app.toml" ]; then
    cp "$PROJECT_ROOT/config/app.toml" "$DEPLOY_DIR/config/app.toml"
    warn "部署目录无 app.toml，已从仓库复制默认配置（请检查 web_auth / download_dir）"
else
    ok "保留现有配置 $DEPLOY_DIR/config/app.toml"
fi

# ----------------- 重启 -----------------
if [ "$DO_RESTART" = true ]; then
    log "启动 $SERVICE_NAME"
    systemctl --user daemon-reload
    systemctl --user restart "$SERVICE_NAME"

    log "等待服务就绪…"
    for i in $(seq 1 30); do
        if curl -fsS "http://127.0.0.1:$HEALTH_PORT/health" >/dev/null 2>&1; then
            ok "健康检查通过（${i}s）"
            break
        fi
        sleep 1
        if [ "$i" = "30" ]; then
            err "健康检查超时，日志尾部："
            journalctl --user -u "$SERVICE_NAME" -n 30 --no-pager
        fi
    done

    systemctl --user --no-pager status "$SERVICE_NAME" | head -12
else
    warn "未重启服务（--no-restart）"
fi

# ----------------- 部署后自检 -----------------
log "认证模式自检："
AUTH_STATUS="$(curl -fsS "http://127.0.0.1:$HEALTH_PORT/api/v1/web-auth/status" 2>/dev/null || echo '{}')"
echo "  $AUTH_STATUS"
case "$AUTH_STATUS" in
    *'"mode":"none"'*)
        warn "认证处于关闭状态（mode=none）。该状态下任何能访问本服务的人"
        warn "都能读写网盘与本机文件。对公网开放前请务必在设置页启用"
        warn "「密码 + 双因素认证」。"
        ;;
    *) ok "认证已启用" ;;
esac

echo
ok "部署完成"
