#!/usr/bin/env python3
"""构建 2016 年至今的日频长周期数据集（穿越牛熊，用于策略稳健性检验）。

与 build_live_dataset.py 的分工，两者**不可混用**：

| 数据集 | 区间 | 标的 | 用途 |
|---|---|---|---|
| live_daily.csv  | 2024-01 ~ 今 | 实盘四只场外基金真实净值 | 账户镜像、严格执行 vs 实际执行 |
| long_daily.csv  | 2016-01 ~ 今 | 三腿长历史代理 | 牛熊分段、多折 walk-forward、≥36 月 holdout、置换检验 |

长周期层必须代理的原因：实盘四只基金中最晚成立的 020341 只有 2024-01 起的
净值，共同区间仅 34 个月且只有单一 regime；用户要求样本穿越牛熊、回溯到 2016。
因此这里为每条腿选择 2016 起即连续可得的**可比标的**，并逐条记录口径差异：

- 纳指腿：539001 建信纳指100 指数 QDII（人民币份额，本身即实盘持仓之一，
  2010 起）。实盘纳指腿由 018966 + 539001 各半合成，长周期层只能用 539001
  单只代表——018966 成立于 2023-08。
- 红利腿：H30269 中证红利低波动**价格指数**（2016 起全覆盖）。实盘 007605
  跟踪的是 930740 沪深300红利低波动，该指数 2018-12 才有数据，无法满足 2016
  起点。价格指数不含股息，会低估该腿绝对收益；但策略与买入持有基准共用同一
  序列，不影响两者的**相对**比较。
- 黄金腿：518880 华安黄金 ETF 场内前复权收盘价（2013 起）。实盘 020341 是
  其场外联接 E 份额，存在现金留存与跟踪误差。

FGI 来源为 GitHub `whit3rabbit/fear-greed-data` 的合并存档（2011-01-03 起日频），
其中 2011-01~2021-01 为第三方冻结存档（整数分值），2021-02 起由 CNN 端点再生成
（浮点分值）。这是**精度断点**，不是数值断点；meta 中逐段标注。

同样采用 point-in-time 对齐：FGI 严格早于交易日，行情不晚于交易日。

用法：python3 build_long_dataset.py [--out long_daily.csv] [--check]
"""

import argparse
import bisect
import csv
import datetime
import io
import json
import os
import sys
import time
import urllib.request

FGI_URL = "https://raw.githubusercontent.com/whit3rabbit/fear-greed-data/main/fear-greed.csv"
KLINE = "https://push2his.eastmoney.com/api/qt/stock/kline/get"
FUND_HOST = "https://fundmobapi.eastmoney.com/FundMNewApi/FundMNHisNetList"
FUND_BASE = "plat=Android&appType=ttjj&product=EFund&Version=1&deviceid=1"
UA = {"User-Agent": "Mozilla/5.0", "Referer": "https://quote.eastmoney.com/"}
FUND_UA = {"User-Agent": "Mozilla/5.0", "Referer": "https://fund.eastmoney.com/"}
START = "2016-01-01"
PAGE_SIZE = 500

LEGS = ["us_stocks", "cn_stocks", "counter_cyclical"]
LEG_META = {
    "us_stocks": {
        "proxy": "539001 建信纳斯达克100指数(QDII) 人民币份额 全收益净值指数",
        "live": "018966 + 539001 各半",
        "caveat": "长周期层用 539001 单只代表纳指腿；018966 成立于 2023-08，无法回溯到 2016。",
    },
    "cn_stocks": {
        "proxy": "H30269 中证红利低波动价格指数（不含股息）",
        "live": "007605 嘉实沪深300红利低波动ETF联接A（跟踪 930740）",
        "caveat": "实盘跟踪的 930740 仅 2018-12 起有数据；此处改用 H30269 以满足 2016 起点。"
                  "指数不同且为价格指数，绝对收益低估股息，但与基准同口径不影响相对比较。",
    },
    "counter_cyclical": {
        "proxy": "518880 华安黄金ETF 场内前复权收盘价",
        "live": "020341 工银黄金ETF联接E",
        "caveat": "场外联接存在现金留存与跟踪误差；长周期层用场内 ETF 价格代理。",
    },
}


def get(url, headers, retries=3, timeout=30):
    last = None
    for i in range(retries):
        try:
            return urllib.request.urlopen(
                urllib.request.Request(url, headers=headers), timeout=timeout).read()
        except Exception as exc:
            last = exc
            time.sleep(1.0 * (i + 1))
    raise RuntimeError(f"抓取失败 {url}: {last}")


def fetch_fgi():
    text = get(FGI_URL, UA).decode("utf-8")
    out, dup = {}, 0
    for row in csv.DictReader(io.StringIO(text)):
        try:
            d = datetime.date.fromisoformat(row["Date"]).isoformat()
            v = float(row["Fear Greed"])
        except (KeyError, ValueError, TypeError):
            continue
        if d in out:
            dup += 1
        out[d] = v  # 后写覆盖，与源仓库的再生成顺序一致
    series = sorted(out.items())
    print(f"       {len(series)} 个日频观测 {series[0][0]} .. {series[-1][0]}（去重 {dup} 条）")
    return series


def fetch_kline(secid):
    """东财日线收盘价，fqt=1 前复权。"""
    url = (f"{KLINE}?secid={secid}&fields1=f1,f2,f3&fields2=f51,f53"
           f"&klt=101&fqt=1&beg=20160101&end=20301231&lmt=6000")
    data = json.loads(get(url, UA))
    out = []
    for line in (data.get("data") or {}).get("klines") or []:
        parts = line.split(",")
        if len(parts) < 2:
            continue
        try:
            out.append((parts[0], float(parts[1])))
        except ValueError:
            continue
    return sorted(out)


def fetch_fund_tri(code):
    """场外基金 JZZZL 日增长率链式复合为全收益指数。"""
    first = json.loads(get(
        f"{FUND_HOST}?FCODE={code}&pageIndex=1&pageSize={PAGE_SIZE}&{FUND_BASE}", FUND_UA))
    total = int(first.get("TotalCount") or 0)
    pages = (total + PAGE_SIZE - 1) // PAGE_SIZE
    rows = {}
    for page in range(1, pages + 1):
        data = first if page == 1 else json.loads(get(
            f"{FUND_HOST}?FCODE={code}&pageIndex={page}&pageSize={PAGE_SIZE}&{FUND_BASE}", FUND_UA))
        for item in data.get("Datas") or []:
            d, raw = item.get("FSRQ"), item.get("JZZZL")
            if not d or raw in (None, "", "--"):
                continue
            try:
                rows[d] = float(raw)
            except ValueError:
                continue
        if page < pages:
            time.sleep(0.3)
    idx, out = 1.0, []
    for d, pct in sorted(rows.items()):
        idx *= 1.0 + pct / 100.0
        out.append((d, idx))
    return out


def build(series_by_leg, fgi):
    # 日历 = 三腿行情日并集，起点取三腿都已有值的首日
    all_dates = sorted(set().union(*(set(d for d, _ in s) for s in series_by_leg.values())))
    start = max(min(d for d, _ in s) for s in series_by_leg.values())
    calendar = [d for d in all_dates if d >= start]
    fgi_dates = [d for d, _ in fgi]
    leg_dates = {k: [d for d, _ in s] for k, s in series_by_leg.items()}
    leg_vals = {k: [v for _, v in s] for k, s in series_by_leg.items()}

    rows = []
    for d in calendar:
        k = bisect.bisect_left(fgi_dates, d) - 1  # 严格早于当日
        fgi_val = fgi[k][1] if k >= 0 else None
        fgi_age = (datetime.date.fromisoformat(d) -
                   datetime.date.fromisoformat(fgi_dates[k])).days if k >= 0 else -1
        prices, stale = [], []
        for leg in LEGS:
            j = bisect.bisect_right(leg_dates[leg], d) - 1
            if j < 0:
                prices.append(None)
                stale.append(-1)
            else:
                prices.append(leg_vals[leg][j])
                stale.append((datetime.date.fromisoformat(d) -
                              datetime.date.fromisoformat(leg_dates[leg][j])).days)
        if any(p is None for p in prices):
            continue
        rows.append({"date": d, "fgi": fgi_val, "fgi_age_days": fgi_age,
                     "prices": prices, "stale": stale})
    # 归一到区间起点 = 1.0
    base = rows[0]["prices"]
    for r in rows:
        r["prices"] = [v / b for v, b in zip(r["prices"], base)]
    return rows


def write(path, rows, fgi, raw_legs):
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("# 2016-至今日频长周期代理数据集（point-in-time）\n")
        fh.write("# 三腿均为可比代理标的，非实盘持仓；口径差异见 long_daily_meta.json\n")
        fh.write("date,fgi,fgi_age_days," + ",".join(LEGS) + ",stale_days,fgi_source\n")
        for r in rows:
            fv = f"{r['fgi']:.4f}" if r["fgi"] is not None else "-1"
            src = "archive" if r["date"] < "2021-02-01" else "cnn_regen"
            fh.write(f"{r['date']},{fv},{r['fgi_age_days']},")
            fh.write(",".join(f"{v:.8f}" for v in r["prices"]) + ",")
            fh.write("|".join(str(x) for x in r["stale"]) + f",{src}\n")

    valid = [r for r in rows if r["fgi"] is not None]
    meta = {
        "generated_at": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "purpose": "穿越牛熊的策略稳健性检验；不得用于实盘收益预测或账户镜像",
        "method": "point-in-time as-of join; FGI strictly before trade date; prices on or before",
        "calendar_range": [rows[0]["date"], rows[-1]["date"]] if rows else None,
        "calendar_observations": len(rows),
        "strategy_ready_range": [valid[0]["date"], valid[-1]["date"]] if valid else None,
        "strategy_ready_observations": len(valid),
        "sources": {
            "fgi": FGI_URL,
            "fgi_range": [fgi[0][0], fgi[-1][0]],
            "fgi_observations": len(fgi),
            "fgi_precision_break": "2021-02-01：之前为第三方冻结存档（整数分值），之后为 CNN 端点再生成（浮点）",
            "quotes": "eastmoney push2his kline (fqt=1) / fundmobapi JZZZL chained total return",
        },
        "legs": {k: {**v,
                     "first": min(d for d, _ in raw_legs[k]),
                     "last": max(d for d, _ in raw_legs[k]),
                     "observations": len(raw_legs[k])}
                 for k, v in LEG_META.items()},
        "regimes": [
            {"name": "2018Q4 全球回调", "from": "2018-10-01", "to": "2018-12-31"},
            {"name": "2020 疫情熔断", "from": "2020-02-19", "to": "2020-03-31"},
            {"name": "2022 纳指熊市", "from": "2022-01-01", "to": "2022-12-31"},
            {"name": "2023-2025 风险资产牛市", "from": "2023-01-01", "to": "2025-10-31"},
            {"name": "2026 纳指与黄金同步回撤", "from": "2026-01-01", "to": "2026-11-06"},
        ],
        "limitations": [
            "三腿均为长历史代理，标的/指数/通道与实盘四只基金不同；"
            "实盘结论请使用 live_daily.csv。",
            "红利腿 H30269 为价格指数，不含股息，绝对收益被低估；"
            "策略与基准同口径，相对比较仍有效。",
            "实盘跟踪的 930740 指数仅 2018-12 起有数据，无法覆盖 2016 起点。",
            "FGI 在 2021-02 前后存在整数/浮点精度断点，非数值断点。",
            "未覆盖 2000-2002 互联网泡沫与 2008 型多年熊市；这两段仍属未经检验区间。",
        ],
    }
    meta_path = os.path.splitext(path)[0] + "_meta.json"
    with open(meta_path, "w", encoding="utf-8") as fh:
        json.dump(meta, fh, ensure_ascii=False, indent=2)
    return meta, meta_path


def check(path, max_age_days=30):
    meta_path = os.path.splitext(path)[0] + "_meta.json"
    if not os.path.exists(path) or not os.path.exists(meta_path):
        return False, "长周期数据集不存在，请运行 `mns backtest update-data`"
    with open(path, encoding="utf-8") as fh:
        last = [ln for ln in fh if ln.strip() and not ln.startswith("#")][-1].split(",")[0]
    age = (datetime.date.today() - datetime.date.fromisoformat(last)).days
    if age > max_age_days:
        return False, f"长周期数据集已陈旧 {age} 天（末日 {last}）"
    return True, f"长周期数据集末日 {last}，落后 {age} 天（SLA {max_age_days} 天）"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(os.path.dirname(__file__), "long_daily.csv"))
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--max-age-days", type=int, default=30)
    args = ap.parse_args()
    if args.check:
        ok, msg = check(args.out, args.max_age_days)
        print(msg)
        sys.exit(0 if ok else 2)

    print("[抓取] CNN FGI 全历史日频 ...", flush=True)
    fgi = fetch_fgi()
    print("[抓取] 纳指腿 539001 ...", flush=True)
    us = fetch_fund_tri("539001")
    print("[抓取] 红利低波腿 H30269 ...", flush=True)
    cn = fetch_kline("2.H30269")
    print("[抓取] 黄金腿 518880 ...", flush=True)
    gold = fetch_kline("1.518880")
    raw = {"us_stocks": us, "cn_stocks": cn, "counter_cyclical": gold}
    for k, v in raw.items():
        print(f"       {k:18s} {len(v)} 日  {v[0][0]} .. {v[-1][0]}")
        if not v:
            raise SystemExit(f"{k} 无数据，终止（不得用估填值代替）")

    rows = build(raw, fgi)
    meta, meta_path = write(args.out, rows, fgi, raw)
    print(f"\n[输出] {args.out}: {meta['calendar_observations']} 行 "
          f"({meta['calendar_range'][0]} .. {meta['calendar_range'][1]})")
    print(f"[输出] {meta_path}")
    print(f"[策略] 可验证区间 {meta['strategy_ready_range']} "
          f"({meta['strategy_ready_observations']} 个交易日)")
    ok, msg = check(args.out, args.max_age_days)
    print(f"[SLA ] {msg}")


if __name__ == "__main__":
    main()
