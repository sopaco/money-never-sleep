#!/usr/bin/env python3
"""构建实盘账户镜像数据集（point-in-time 日频，无前视偏差）。

与 build_dataset.py（长周期**代理**数据，2016-2025 月频）的区别：

1. 标的改为用户实盘持有的四只场外基金，不再用 270042/510880/518880 代理。
2. 日频而非月频，保留月内低点，使最大回撤可按真实日度口径计算。
3. **point-in-time 对齐**：每一行的 FGI 取「严格早于该日」的最后一条已公布值
   （CNN 于美股收盘后计算，北京时间次日才可得），净值取「不晚于该日」的
   最后一条已披露值（QDII 净值 T+1/T+2 披露，缺口沿用前值）。因此任何一行
   都不含当日收盘后才知道的信息，回测不可能"用月末情绪按月末净值同刻成交"。
4. 记录 stale 天数与披露日，供成本/结算模型判断信号是否可执行。

数据源：东方财富 fundmobapi（JZZZL 日增长率链式复合，规避份额折算断点）、
CNN Fear & Greed graphdata（约一年日频历史）。

用法：
    python3 build_live_dataset.py [--out live_daily.csv] [--meta dataset_meta.json]
    python3 build_live_dataset.py --check        # 只校验现有数据集新鲜度
"""

import argparse
import bisect
import datetime
import json
import os
import sys
import time
import urllib.request
from collections import OrderedDict

FUND_HOST = "https://fundmobapi.eastmoney.com/FundMNewApi/FundMNHisNetList"
FUND_BASE = "plat=Android&appType=ttjj&product=EFund&Version=1&deviceid=1"
CNN_URL = "https://production.dataviz.cnn.io/index/fearandgreed/graphdata"
HEADERS = {"User-Agent": "Mozilla/5.0", "Referer": "https://fund.eastmoney.com/"}
CNN_HEADERS = {
    "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                  "AppleWebKit/537.36 (KHTML, like Gecko) "
                  "Chrome/120.0.0.0 Safari/537.36",
    "Referer": "https://www.cnn.com/markets/fear-and-greed",
    "Accept": "application/json, text/plain, */*",
}

# 用户实盘持仓（reports/2026-11-08.txt）。顺序与 allocation 的三腿对应：
# 纳指腿由两只 QDII 合成，红利低波腿与黄金腿各一只场外联接。
# weight 为腿内权重，来自实盘持仓市值比；可由 --holdings 覆盖。
LEGS = OrderedDict([
    ("018966", {"name": "汇添富纳斯达克100ETF联接", "leg": "us_stocks", "weight": 0.5,
                "channel": "qdii_feeder"}),
    ("539001", {"name": "建信纳斯达克100指数(QDII)", "leg": "us_stocks", "weight": 0.5,
                "channel": "qdii_feeder"}),
    ("007605", {"name": "嘉实沪深300红利低波动ETF联接A", "leg": "cn_stocks", "weight": 1.0,
                "channel": "cn_feeder"}),
    ("020341", {"name": "工银黄金ETF联接E", "leg": "counter_cyclical", "weight": 1.0,
                "channel": "cn_feeder"}),
])
PAGE_SIZE = 500


def fetch_json(url, headers, retries=3, timeout=30):
    last = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, headers=headers)
            return json.loads(urllib.request.urlopen(req, timeout=timeout).read())
        except Exception as exc:
            last = exc
            time.sleep(1.0 * (attempt + 1))
    raise RuntimeError(f"抓取失败 {url}: {last}")


def fetch_nav(code):
    """返回 [(date, 全收益指数, 单位净值)]，按日期升序。

    全收益指数用 JZZZL 链式复合（已含分红与份额折算调整），供策略回测；
    单位净值 DWJZ 供账户镜像按「真实份额 × 当日净值」计算市值——两者量纲
    不同，不能用全收益指数乘实盘份额。
    """
    first = fetch_json(f"{FUND_HOST}?FCODE={code}&pageIndex=1&pageSize={PAGE_SIZE}&{FUND_BASE}",
                       HEADERS)
    total = int(first.get("TotalCount") or 0)
    if total <= 0:
        raise RuntimeError(f"{code} 无历史净值")
    pages = (total + PAGE_SIZE - 1) // PAGE_SIZE
    rows = {}
    for page in range(1, pages + 1):
        data = first if page == 1 else fetch_json(
            f"{FUND_HOST}?FCODE={code}&pageIndex={page}&pageSize={PAGE_SIZE}&{FUND_BASE}",
            HEADERS)
        for item in data.get("Datas") or []:
            date, raw = item.get("FSRQ"), item.get("JZZZL")
            if not date or raw in (None, "", "--"):
                continue
            try:
                dwjz = float(item.get("DWJZ"))
                rows[date] = (float(raw), dwjz)
            except (ValueError, TypeError):
                continue
        if page < pages:
            time.sleep(0.3)
    series = sorted(rows.items())
    index, out = 1.0, []
    for date, (pct, dwjz) in series:
        index *= 1.0 + pct / 100.0
        out.append((date, index, dwjz))
    return out


def fetch_fgi():
    """CNN 近约一年日频 FGI，返回 [(date, score)] 升序。"""
    data = fetch_json(CNN_URL, CNN_HEADERS)
    points = data.get("fear_and_greed_historical", {}).get("data", [])
    out = []
    for p in points:
        try:
            date = datetime.date.fromtimestamp(p["x"] / 1000).isoformat()
            out.append((date, float(p["y"])))
        except (KeyError, TypeError, ValueError):
            continue
    return sorted(set(out))


def build(nav, fgi):
    """按 point-in-time 规则对齐，返回 (rows, meta_rows)。"""
    # 交易日历 = 四只基金净值日的并集；起点取四只都有净值的首日
    all_dates = sorted(set().union(*(set(d for d, _, _ in s) for s in nav.values())))
    start = max(min(d for d, _, _ in s) for s in nav.values())
    calendar = [d for d in all_dates if d >= start]

    fgi_dates = [d for d, _ in fgi]
    # 净值 as-of：把每只基金的 (日期, 值) 展平，用二分找 <= 当日 的最后一条
    nav_dates = {c: [d for d, _, _ in s] for c, s in nav.items()}
    nav_vals = {c: [v for _, v, _ in s] for c, s in nav.items()}
    unit_vals = {c: [u for _, _, u in s] for c, s in nav.items()}

    rows = []
    for d in calendar:
        # FGI 严格早于当日（前一交易日及以前已公布值），杜绝前视
        k = bisect.bisect_left(fgi_dates, d) - 1
        fgi_val = fgi[k][1] if k >= 0 else None
        fgi_age = (datetime.date.fromisoformat(d) -
                   datetime.date.fromisoformat(fgi_dates[k])).days if k >= 0 else -1
        prices, stale, units = [], [], []
        for code in LEGS:
            j = bisect.bisect_right(nav_dates[code], d) - 1
            if j < 0:
                prices.append(None)
                stale.append(-1)
                units.append(None)
                continue
            prices.append(nav_vals[code][j])
            units.append(unit_vals[code][j])
            stale.append((datetime.date.fromisoformat(d) -
                          datetime.date.fromisoformat(nav_dates[code][j])).days)
        if any(p is None for p in prices) or any(u is None for u in units):
            continue
        rows.append({
            "date": d,
            "fgi": fgi_val,
            "fgi_age_days": fgi_age,
            "nav": [round(p, 8) for p in prices],
            "unit_nav": [round(u, 6) for u in units],
            "nav_stale_days": stale,
        })
    return rows


def write(path, rows, nav, fgi):
    codes = list(LEGS)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("# point-in-time 日频实盘数据集；fgi=-1 表示该日无可用的已公布 FGI\n")
        fh.write("# 前 4 列为全收益指数(回测用)，末 4 列为真实单位净值(账户镜像按份额估值用)\n")
        fh.write("date,fgi,fgi_age_days," + ",".join(codes) + ",nav_stale_days,"
                 + ",".join(c + "_unit" for c in codes) + "\n")
        for r in rows:
            fh.write(f"{r['date']},{r['fgi']:.4f}" if r["fgi"] is not None
                     else f"{r['date']},-1")
            fh.write(f",{r['fgi_age_days']},")
            fh.write(",".join(f"{v:.8f}" for v in r["nav"]) + ",")
            fh.write("|".join(str(x) for x in r["nav_stale_days"]) + ",")
            fh.write(",".join(f"{v:.6f}" for v in r["unit_nav"]) + "\n")

    valid = [r for r in rows if r["fgi"] is not None]
    meta = {
        "generated_at": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "method": "point-in-time as-of join; FGI strictly before trade date; NAV on or before",
        "sources": {
            "nav": "eastmoney fundmobapi FundMNHisNetList (JZZZL chained total-return index)",
            "fgi": "CNN Fear & Greed graphdata (daily, ~1y history)",
        },
        "legs": {c: {**v,
                     "nav_first": min(d for d, _, _ in nav[c]),
                     "nav_last": max(d for d, _, _ in nav[c]),
                     "nav_observations": len(nav[c])}
                 for c, v in LEGS.items()},
        "fgi_range": [fgi[0][0], fgi[-1][0]] if fgi else None,
        "fgi_observations": len(fgi),
        "calendar_range": [rows[0]["date"], rows[-1]["date"]] if rows else None,
        "calendar_observations": len(rows),
        "strategy_ready_range": [valid[0]["date"], valid[-1]["date"]] if valid else None,
        "strategy_ready_observations": len(valid),
        "limitations": [
            "CNN 仅提供约一年日频 FGI，故策略可验证区间不足 36 个月；"
            "更长历史的 holdout/压力段须使用代理数据并单独标注。",
            "四只基金成立时间不同，共同起点由最晚成立的 020341 决定。",
            "QDII 净值披露滞后，缺口按前值延续；这是实盘可知信息的真实约束，非插值。",
            "月度 2016-2025 代理序列仍保留为长周期参考，标的与指数均不同于实盘，"
            "不得用于实盘结论。",
        ],
    }
    meta_path = os.path.splitext(path)[0] + "_meta.json"
    with open(meta_path, "w", encoding="utf-8") as fh:
        json.dump(meta, fh, ensure_ascii=False, indent=2)
    return meta, meta_path


def check(path, max_age_days=30):
    """数据新鲜度 SLA 校验，返回 (ok, message)。"""
    meta_path = os.path.splitext(path)[0] + "_meta.json"
    if not os.path.exists(path) or not os.path.exists(meta_path):
        return False, "实盘数据集不存在，请先运行 `mns backtest update-data`"
    with open(path, encoding="utf-8") as fh:
        last = [ln for ln in fh if ln.strip() and not ln.startswith("#")][-1].split(",")[0]
    age = (datetime.date.today() - datetime.date.fromisoformat(last)).days
    if age > max_age_days:
        return False, f"实盘数据集已陈旧 {age} 天（末日 {last}，SLA {max_age_days} 天）"
    return True, f"实盘数据集末日 {last}，落后 {age} 天（SLA {max_age_days} 天）"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(os.path.dirname(__file__), "live_daily.csv"))
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--max-age-days", type=int, default=30)
    args = ap.parse_args()

    if args.check:
        ok, msg = check(args.out, args.max_age_days)
        print(msg)
        sys.exit(0 if ok else 2)

    nav = {}
    for code, info in LEGS.items():
        print(f"[抓取] {code} {info['name']} ...", flush=True)
        nav[code] = fetch_nav(code)
        print(f"        {len(nav[code])} 个净值日  {nav[code][0][0]} .. {nav[code][-1][0]}")
    print("[抓取] CNN FGI ...", flush=True)
    fgi = fetch_fgi()
    print(f"        {len(fgi)} 个观测  {fgi[0][0]} .. {fgi[-1][0]}")

    rows = build(nav, fgi)
    meta, meta_path = write(args.out, rows, nav, fgi)
    print(f"\n[输出] {args.out}: {meta['calendar_observations']} 行 "
          f"({meta['calendar_range'][0]} .. {meta['calendar_range'][1]})")
    print(f"[输出] {meta_path}")
    print(f"[策略] 可验证区间 {meta['strategy_ready_range']} "
          f"({meta['strategy_ready_observations']} 个交易日)")
    ok, msg = check(args.out, args.max_age_days)
    print(f"[SLA ] {msg}")


if __name__ == "__main__":
    main()
