#!/usr/bin/env python3
"""沪深300 单资产策略实验数据集（2016 至今日频，point-in-time）。

与 long_daily.csv 共用同一套 point-in-time 构建逻辑和 FGI 来源，因此可以直接
送进同一套审计引擎，保证「信号、成本、执行滞后、回撤口径」完全可比。

单资产的表达方式：三腿列都填同一个 510300 前复权净值序列。由于三腿完全相关，
55/25/20 的内部权重不会引发腿间切换；调用方再把 allocation 覆盖为 (1,0,0)，
组合即严格等价于「沪深300 × 目标仓位 + 现金」。这不是伪造三个资产。

标的口径：510300 华泰柏瑞沪深300ETF 前复权收盘价，含分红再投资近似；
对照 000300 价格指数不含股息，仅用于校验口径差异，不进入回测。

用法：
  python3 build_hs300_dataset.py            # 抓取并重建
  python3 build_hs300_dataset.py --check    # 只校验新鲜度
"""

import argparse
import datetime
import json
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_long_dataset import FGI_URL, LEGS, build, fetch_fgi, fetch_kline  # noqa: E402

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "hs300_daily.csv")
META = os.path.join(os.path.dirname(os.path.abspath(__file__)), "hs300_daily_meta.json")

ETF = "1.510300"
INDEX = "1.000300"

# A 股自身的主要牛熊区间，按市场共识划分（非算法识别）。
# 每段的实际买入持有涨跌会在生成时打印并写入 meta，便于读者自行核验分段是否合理。
REGIMES = [
    {"name": "2016-2017 蓝筹慢牛", "from": "2016-01-04", "to": "2018-01-24"},
    {"name": "2018 贸易摩擦熊市", "from": "2018-01-25", "to": "2019-01-03"},
    {"name": "2019-2021 核心资产牛市", "from": "2019-01-04", "to": "2021-02-10"},
    {"name": "2021-2024 三年长熊", "from": "2021-02-18", "to": "2024-02-05"},
    {"name": "2024-2025 政策反弹", "from": "2024-09-24", "to": "2025-12-31"},
    {"name": "2026 回调", "from": "2026-01-05", "to": "2026-11-06"},
]


def pearson(xs, ys):
    n = min(len(xs), len(ys))
    if n < 30:
        return None
    mx = sum(xs[:n]) / n
    my = sum(ys[:n]) / n
    num = sum((xs[i] - mx) * (ys[i] - my) for i in range(n))
    dx = math.sqrt(sum((x - mx) ** 2 for x in xs[:n]))
    dy = math.sqrt(sum((y - my) ** 2 for y in ys[:n]))
    return num / (dx * dy) if dx > 0 and dy > 0 else None


def max_drawdown(vals):
    peak, dd = vals[0], 0.0
    for v in vals:
        peak = max(peak, v)
        dd = min(dd, v / peak - 1.0)
    return dd


def diagnostics(rows, closes, dates, horizon=20):
    """FGI 对 A 股未来收益是否有可辨识的线性预测力。"""
    fgi = [r["fgi"] for r in rows]
    fwd = [closes[i + horizon] / closes[i] - 1.0 if i + horizon < len(closes) else None
           for i in range(len(rows))]
    pairs = [(fgi[i], fwd[i]) for i in range(len(rows))
             if fgi[i] is not None and fwd[i] is not None]
    corr = pearson([p[0] for p in pairs], [p[1] for p in pairs])

    buckets = {"<20": [], "20-40": [], "40-55": [], "55-75": [], ">=75": []}
    for f, r in pairs:
        k = "<20" if f < 20 else "20-40" if f < 40 else "40-55" if f < 55 else "55-75" if f < 75 else ">=75"
        buckets[k].append(r)
    by_bucket = {k: {"samples": len(v), "avg_fwd_20d_pct": round(sum(v) / len(v) * 100, 3)}
                 for k, v in buckets.items() if v}

    total = closes[-1] / closes[0] - 1.0
    days = (datetime.date.fromisoformat(dates[-1]) - datetime.date.fromisoformat(dates[0])).days
    return {
        "price_total_return_pct": round(total * 100, 2),
        "price_annualized_pct": round(((1 + total) ** (365.0 / days) - 1) * 100, 2),
        "price_max_drawdown_pct": round(max_drawdown(closes) * 100, 2),
        "observations": len(rows),
        "fgi_vs_forward_20d_corr": round(corr, 4) if corr is not None else None,
        "fgi_vs_forward_20d_pairs": len(pairs),
        "forward_20d_return_by_fgi_bucket": by_bucket,
    }


def check(path, max_age_days=30):
    if not os.path.exists(path) or not os.path.exists(META):
        return False, "沪深300 实验数据集不存在，请先运行 build_hs300_dataset.py"
    with open(path, encoding="utf-8") as fh:
        last = [ln for ln in fh if ln.strip() and not ln.startswith("#")][-1].split(",")[0]
    age = (datetime.date.today() - datetime.date.fromisoformat(last)).days
    if age > max_age_days:
        return False, f"沪深300 实验数据集已陈旧 {age} 天（末日 {last}）"
    return True, f"沪深300 实验数据集末日 {last}，落后 {age} 天（SLA {max_age_days} 天）"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--max-age-days", type=int, default=30)
    args = ap.parse_args()
    if args.check:
        ok, msg = check(args.out, args.max_age_days)
        print(msg)
        sys.exit(0 if ok else 2)

    print("[抓取] CNN FGI 全历史日频 ...", flush=True)
    fgi = fetch_fgi()
    print("[抓取] 510300 沪深300ETF 前复权 ...", flush=True)
    etf = fetch_kline(ETF)
    print("[抓取] 000300 沪深300价格指数（仅校验） ...", flush=True)
    idx = fetch_kline(INDEX)
    for name, s in [("510300", etf), ("000300", idx)]:
        print(f"       {name} {len(s)} 日 {s[0][0]} .. {s[-1][0]}")
    if not etf:
        raise SystemExit("510300 无数据，终止（不得用估填值代替）")

    # 三腿注入同一序列；配合调用方 allocation=(1,0,0) 严格等价于单资产
    rows = build({k: list(etf) for k in LEGS}, fgi)
    if not rows:
        raise SystemExit("point-in-time 构建结果为空")

    dates = [r["date"] for r in rows]
    closes = [r["prices"][0] for r in rows]
    diag = diagnostics(rows, closes, dates)

    with open(args.out, "w", encoding="utf-8") as fh:
        fh.write("# 沪深300 单资产策略实验数据集（2016-至今日频，point-in-time）\n")
        fh.write("# 三腿列为同一个 510300 前复权净值：配合 allocation=(1,0,0) 等价于单资产\n")
        fh.write("# FGI 严格早于交易日；口径与 long_daily.csv 完全一致\n")
        fh.write("date,fgi,fgi_age_days," + ",".join(LEGS) + ",stale_days,fgi_source\n")
        for r in rows:
            fv = f"{r['fgi']:.4f}" if r["fgi"] is not None else "-1"
            src = "archive" if r["date"] < "2021-02-01" else "cnn_regen"
            fh.write(f"{r['date']},{fv},{r['fgi_age_days']},")
            fh.write(",".join(f"{v:.8f}" for v in r["prices"]) + ",")
            fh.write("|".join(str(x) for x in r["stale"]) + f",{src}\n")

    # 各牛熊段的买入持有涨跌，用于核验分段是否合理
    dmap = dict(etf)
    regime_check = []
    for r in REGIMES:
        ds = [d for d in dates if r["from"] <= d <= r["to"]]
        if len(ds) < 2:
            continue
        a, b = dmap.get(ds[0]), dmap.get(ds[-1])
        if a and b:
            regime_check.append({"name": r["name"], "buy_hold_pct": round((b / a - 1) * 100, 2)})

    idx_first, idx_last = idx[0][1], idx[-1][1]
    meta = {
        "generated_at": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "label": "沪深300 单资产实验（510300 前复权，含分红）",
        "purpose": "检验 MNS 逆向择时框架用于单一 A 股宽基的效果；属对照实验，不构成投资建议",
        "method": "point-in-time as-of join；复用 build_long_dataset.build；FGI 严格早于交易日",
        "single_asset": True,
        "required_allocation": {"us_stocks": 1.0, "cn_stocks": 0.0, "counter_cyclical": 0.0},
        "calendar_range": [dates[0], dates[-1]],
        "calendar_observations": len(rows),
        "strategy_ready_range": [dates[0], dates[-1]],
        "strategy_ready_observations": len(rows),
        "sources": {
            "fgi": FGI_URL,
            "fgi_range": [fgi[0][0], fgi[-1][0]],
            "fgi_observations": len(fgi),
            "quote": "eastmoney push2his kline secid=1.510300 fqt=1（前复权，含分红再投资近似）",
            "index_check": "eastmoney 1.000300 价格指数（不含股息），仅用于口径校验",
        },
        "legs": {
            "single": {
                "proxy": "510300 华泰柏瑞沪深300ETF 前复权收盘价（归一到区间起点 1.0）",
                "first": etf[0][0], "last": etf[-1][0], "observations": len(etf),
                "caveat": "三腿列填同一序列，配合 allocation=(1,0,0) 使用；不是三个独立资产。",
            }
        },
        "regimes": REGIMES,
        "regime_buy_hold_check": regime_check,
        "diagnostics": diag,
        "limitations": [
            "单资产实验：不等于用户的三资产组合，结论不可直接迁移到实盘配置。",
            "510300 前复权价格会随未来分红重算历史值，属标准复权口径；比例收益序列的时序信息不受影响。",
            "ETF 场内价格与场外联接基金存在跟踪误差、现金留存与申赎摩擦，本实验按场内 T+1 通道建模。",
            "CNN 恐贪指数刻画美股风险偏好，与 A 股的同步性/预测力有限；meta.diagnostics 给出实测相关性。",
            "牛熊分段按 A 股市场共识划定的日历区间，非算法识别；各段买入持有涨跌见 regime_buy_hold_check。",
            "未覆盖 2008 / 2015 中段等更早期极端行情；2016 起点为用户指定。",
        ],
    }
    with open(META, "w", encoding="utf-8") as fh:
        json.dump(meta, fh, ensure_ascii=False, indent=1)

    print(f"\n[输出] {args.out}: {len(rows)} 行 ({dates[0]} .. {dates[-1]})")
    print(f"[输出] {META}")
    print(f"[校验] 510300 全期 {diag['price_total_return_pct']:+.2f}% / 年化 "
          f"{diag['price_annualized_pct']:+.2f}% / 最大回撤 {diag['price_max_drawdown_pct']:.2f}%")
    print(f"[校验] 000300 价格指数全期 {(idx_last / idx_first - 1) * 100:+.2f}%（不含股息）")
    print(f"[诊断] FGI 与未来 20 日收益相关系数 = {diag['fgi_vs_forward_20d_corr']} "
          f"(n={diag['fgi_vs_forward_20d_pairs']})")
    print("[诊断] 各 FGI 分档的未来 20 日平均收益：")
    for k, v in diag["forward_20d_return_by_fgi_bucket"].items():
        print(f"        {k:6s} n={v['samples']:4d}  {v['avg_fwd_20d_pct']:+.3f}%")
    print("[诊断] 各牛熊段买入持有：")
    for r in regime_check:
        print(f"        {r['name']:22s} {r['buy_hold_pct']:+7.2f}%")
    ok, msg = check(args.out, args.max_age_days)
    print(f"[SLA ] {msg}")


if __name__ == "__main__":
    main()
