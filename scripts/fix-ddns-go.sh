#!/bin/bash
# fix-ddns-go.sh — 一次性修复 ddns-go service 失效
#
# 故障: 二进制目录 2026-06-15 从 /mnt/ssd/codespace/ddns-go/
#       搬到 /mnt/ssd/codespace/tool/ddns-go/,systemd unit 没同步,
#       导致 ConditionFileIsExecutable 失败,服务 2 个半月没起来。
#
# 跑法: sudo bash scripts/fix-ddns-go.sh
#
# 改两处:
#   1. ConditionFileIsExecutable  → 新二进制路径
#   2. ExecStart                   → 新二进制路径 + 正确 config 路径
# 然后 daemon-reload + enable + restart。

set -e

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
log() { echo -e "${BLUE}[$(date '+%H:%M:%S')]${NC} $*"; }
ok()  { echo -e "${GREEN}✅ $*${NC}"; }
warn(){ echo -e "${YELLOW}⚠️  $*${NC}"; }
err() { echo -e "${RED}❌ $*${NC}"; }

# 必须在 root 下运行(改 /etc/systemd/system 才生效)
if [ "$(id -u)" -ne 0 ]; then
    err "请用 sudo 跑: sudo bash $0"
    exit 1
fi

UNIT="/etc/systemd/system/ddns-go.service"
NEW_BIN="/mnt/ssd/codespace/tool/ddns-go/ddns-go"
NEW_CONFIG="/opt/ddns-go/.ddns_go_config.yaml"
OLD_BIN="/mnt/ssd/codespace/ddns-go/ddns-go"
OLD_CONFIG="/root/.ddns_go_config.yaml"

# 预检:必须存在的东西缺一不可
[ -x "$NEW_BIN" ]    || { err "未找到新二进制 $NEW_BIN"; exit 1; }
[ -f "$NEW_CONFIG" ] || { err "未找到 config $NEW_CONFIG"; exit 1; }
[ -f "$UNIT" ]       || { err "未找到 systemd unit $UNIT"; exit 1; }

# 备份原 unit
BAK="${UNIT}.bak.$(date +%Y%m%d-%H%M%S)"
cp "$UNIT" "$BAK"
log "已备份 $UNIT → $BAK"

# 改路径(用 sed 替换两条死路径)
sed -i \
    -e "s|$OLD_BIN|$NEW_BIN|g" \
    -e "s|$OLD_CONFIG|$NEW_CONFIG|g" \
    "$UNIT"

log "已更新 unit:"
grep -E "(ConditionFileIsExecutable|ExecStart)" "$UNIT" || true

# 重新加载 + 启动
systemctl daemon-reload
systemctl enable ddns-go.service
systemctl restart ddns-go.service

# 等几秒确认
sleep 2
if systemctl is-active --quiet ddns-go.service; then
    ok "ddns-go 已成功启动"
    systemctl --no-pager status ddns-go.service | head -10
else
    err "ddns-go 启动失败,查看: journalctl -u ddns-go -n 50"
    exit 1
fi

# 顺便确认监听端口(默认 9876)
sleep 1
if ss -lnt 2>/dev/null | awk '{print $4}' | grep -q ":9876\$"; then
    ok "Web UI 已监听 :9876 → http://localhost:9876"
else
    warn "未检测到 :9876 端口,可能服务还在启动或监听了别的端口"
fi

echo ""
echo -e "${BLUE}接下来:${NC}"
echo "  1. 浏览器打开 http://localhost:9876 验证配置(域名/provider)"
echo "  2. 看一眼 hamr.top 上 jetson-local.hamr.top 的解析是否被刷新成 103.167.26.53"
echo "  3. 重启 systemd 后会自动起来,不再需要手动"
