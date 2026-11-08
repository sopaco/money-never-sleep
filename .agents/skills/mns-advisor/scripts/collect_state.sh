#!/usr/bin/env bash
# MNS 只读状态采集：供 mns-advisor 的 Step 1 使用。
# 严禁在此脚本中加入 today / report / update-prices / buy / sell / cash 写入类命令。
set -uo pipefail

usage() {
  cat <<'USAGE'
用法: collect_state.sh [--monthly <金额>] [--mirror]
  --monthly  额外输出只读的 `mns plan --monthly <金额>`（未提供则跳过）
  --mirror   额外输出只读的 `mns backtest mirror`（纪律执行对照）
  MNS_BIN    环境变量，覆盖 mns 二进制路径
本脚本只执行只读命令：不写快照、不刷价、不改台账。
USAGE
}

MONTHLY=""
WITH_MIRROR=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --monthly)
      [[ $# -ge 2 ]] || { echo "缺少 --monthly 的金额" >&2; exit 2; }
      MONTHLY="$2"; shift 2 ;;
    --mirror) WITH_MIRROR=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数: $1" >&2; usage; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
MNS_BIN="${MNS_BIN:-mns}"
if ! command -v "$MNS_BIN" >/dev/null 2>&1; then
  if [[ -x "$PROJECT_ROOT/target/release/mns" ]]; then
    MNS_BIN="$PROJECT_ROOT/target/release/mns"
  else
    echo "找不到 mns 二进制；设置 MNS_BIN 或先执行 cargo build --release" >&2
    exit 1
  fi
fi

run_section() {
  local title="$1"; shift
  printf '\n===== %s =====\n' "$title"
  "$@" 2>&1 || printf '[采集失败] %s\n' "$*"
}

printf 'MNS 只读状态采集 | %s\n' "$(date '+%Y-%m-%d %H:%M:%S %z')"
printf '二进制: %s | 项目根: %s\n' "$MNS_BIN" "$PROJECT_ROOT"

run_section "生效配置" "$MNS_BIN" config
run_section "现金与流水" "$MNS_BIN" cash
run_section "持仓概览" "$MNS_BIN" portfolio
run_section "组合绩效（只读）" "$MNS_BIN" performance
run_section "最近 30 条流水" "$MNS_BIN" history 30
run_section "买入冷静期（只读）" "$MNS_BIN" guard

if [[ -n "$MONTHLY" ]]; then
  run_section "月度定投计划（只读）" "$MNS_BIN" plan --monthly "$MONTHLY"
else
  printf '\n===== 月度定投计划 =====\n未传 --monthly，跳过（当前配置 cashflow.monthly = 0）\n'
fi

if [[ "$WITH_MIRROR" -eq 1 ]]; then
  run_section "账户镜像（只读）" "$MNS_BIN" backtest mirror
fi

printf '\n===== 三层数据新鲜度 =====\n'
SLA="$(grep -E '^freshness_sla_days[[:space:]]*=' "$HOME/.mns/config.toml" 2>/dev/null | head -1 | tr -dc '0-9')"
SLA="${SLA:-30}"
python3 - "$PROJECT_ROOT" "$SLA" <<'PY'
import datetime, json, pathlib, sys
root, sla = pathlib.Path(sys.argv[1]), int(sys.argv[2])
today = datetime.date.today()
base = root / ".agents" / "skills" / "mns-backtest" / "data"
for name in ("live_daily", "long_daily", "hs300_daily"):
    try:
        meta = json.loads((base / f"{name}_meta.json").read_text())
        end = meta["calendar_range"][1]
        age = (today - datetime.date.fromisoformat(end)).days
        state = "OK" if age <= sla else "STALE"
        print(f"  {name}: 末日 {end}，落后 {age} 天（SLA {sla} 天）[{state}]")
    except Exception as exc:
        print(f"  {name}: 读取失败 {exc}")
PY

printf '\n采集完成。本脚本未执行任何写操作。\n'
printf '决策版报告（mns today / mns report）会写快照与报告文件，须在 Step 3 且用户知情后单独运行。\n'
