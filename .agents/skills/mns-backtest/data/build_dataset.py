#!/usr/bin/env python3
"""重建 MNS 回测数据集（真实全收益序列）。

背景：旧版 monthly_real_final.csv 中 dividend_low_vol / gold_cny 两列为人工估填
（112/112 全整数、黄金列 108/112 可被5整除），且 dividend_low_vol 在 2019-01
存在 3200 -> 100 的序列拼接断点，凭空产生 -97% 暴跌。本脚本改为抓取真实数据。

方法：不使用 DWJZ（单位净值）或 LJJZ（累计净值）——这两个字段在发生份额折算的
基金上会不一致（如 518880 二者相差 2.6 倍）。改用官方每日净值增长率 JZZZL 链式
复合成全收益指数，该字段按构造已对分红与份额折算做过调整。

数据源：东方财富 fundmobapi（本环境下唯一可达的行情主机；Yahoo 与 push2his 均被拦截）。

用法：python3 build_dataset.py [--out monthly_total_return.csv]
"""

import argparse
import json
import time
import urllib.request
from collections import OrderedDict

BASE = "plat=Android&appType=ttjj&product=EFund&Version=1&deviceid=1"
HOST = "https://fundmobapi.eastmoney.com/FundMNewApi/FundMNHisNetList"
HEADERS = {
    "User-Agent": "Mozilla/5.0",
    "Referer": "https://fund.eastmoney.com/",
}

# 三条资产腿。全部以人民币计价，故美股腿选用 QDII 人民币份额——
# 其净值已内含汇率折算，避免旧数据集把美元计价的 QQQ 直接混入人民币组合。
LEGS = OrderedDict([
    ("us_stocks",        ("270042", "广发纳斯达克100ETF联接人民币QDII")),
    ("cn_stocks",        ("510880", "红利ETF华泰柏瑞")),
    ("counter_cyclical", ("518880", "黄金ETF华安")),
])

PAGE_SIZE = 500


def fetch_page(code, page, size=PAGE_SIZE, retries=3):
    url = f"{HOST}?FCODE={code}&pageIndex={page}&pageSize={size}&{BASE}"
    last = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, headers=HEADERS)
            return json.loads(urllib.request.urlopen(req, timeout=30).read())
        except Exception as exc:  # 网络抖动重试
            last = exc
            time.sleep(1.0 * (attempt + 1))
    raise RuntimeError(f"抓取 {code} 第 {page} 页失败: {last}")


def fetch_daily_returns(code):
    """返回 [(date, daily_return_pct)]，按日期升序。"""
    first = fetch_page(code, 1)
    total = int(first.get("TotalCount") or 0)
    if total <= 0:
        raise RuntimeError(f"{code} 无历史数据")
    pages = (total + PAGE_SIZE - 1) // PAGE_SIZE

    rows = {}
    for page in range(1, pages + 1):
        data = first if page == 1 else fetch_page(code, page)
        for item in data.get("Datas") or []:
            date = item.get("FSRQ")
            raw = item.get("JZZZL")
            if not date or raw in (None, "", "--"):
                continue
            try:
                rows[date] = float(raw)
            except ValueError:
                continue
        if page < pages:
            time.sleep(0.35)  # 温和限速
    return sorted(rows.items())


def chain_link(daily):
    """把每日增长率链式复合成全收益指数（起点 = 1.0）。"""
    index, out = 1.0, []
    for date, pct in daily:
        index *= 1.0 + pct / 100.0
        out.append((date, index))
    return out


def to_month_end(series):
    """每月取最后一个交易日，返回 OrderedDict[YYYY-MM] = value。"""
    monthly = OrderedDict()
    for date, value in series:
        monthly[date[:7]] = value  # 已升序，后写覆盖 = 月末
    return monthly


def load_fgi_monthly(paths):
    """读取现有 FGI 日度 CSV，聚合为月末值。"""
    monthly = OrderedDict()
    rows = []
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                parts = line.strip().split(",")
                if len(parts) < 2:
                    continue
                try:
                    rows.append((parts[0], float(parts[1])))
                except ValueError:
                    continue  # 表头
    for date, score in sorted(rows):
        monthly[date[:7]] = score
    return monthly


CNN_URL = "https://production.dataviz.cnn.io/index/fearandgreed/graphdata"


def fetch_cnn_recent_fgi():
    """抓取 CNN 近约一年的日度 FGI，用于构建样本外 holdout 区块。

    注意：CNN 仅提供约一年历史，与仓库内 FGI CSV（截至 2025-04）之间存在
    约 3.5 个月空档，无法从任何可达数据源补齐。因此近端数据只作为独立的
    holdout 区块单独回测，不与主序列拼接——拼接会在复利上产生断点。
    """
    req = urllib.request.Request(CNN_URL, headers={
        "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                      "AppleWebKit/537.36 (KHTML, like Gecko) "
                      "Chrome/120.0.0.0 Safari/537.36",
        "Referer": "https://www.cnn.com/markets/fear-and-greed",
        "Accept": "application/json, text/plain, */*",
        "Accept-Language": "en-US,en;q=0.9",
        "Connection": "keep-alive",
    })
    import datetime
    data = json.loads(urllib.request.urlopen(req, timeout=30).read())
    out = OrderedDict()
    for point in data.get("fear_and_greed_historical", {}).get("data", []):
        date = datetime.date.fromtimestamp(point["x"] / 1000).isoformat()
        out[date[:7]] = float(point["y"])  # 升序覆盖 = 月末
    return out


def write_dataset(path, months, fgi, legs):
    """各腿在区间起点归一化为 1.0 后写出。"""
    bases = {k: legs[k][months[0]] for k in legs}
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("date,fgi," + ",".join(legs) + "\n")
        for month in months:
            vals = [f"{legs[k][month] / bases[k]:.6f}" for k in legs]
            fh.write(f"{month},{fgi[month]:.2f}," + ",".join(vals) + "\n")
    print(f"\n[输出] {path}: {len(months)} 个月 ({months[0]} .. {months[-1]})")
    for key in legs:
        total = legs[key][months[-1]] / bases[key]
        print(f"       {key:16s} 期间全收益 {total:.3f}x "
              f"({(total - 1) * 100:+.0f}%)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="monthly_total_return.csv")
    ap.add_argument("--holdout-out", default="monthly_total_return_holdout.csv")
    ap.add_argument("--fgi", nargs="*",
                    default=["fgi_2016_2020.csv", "fgi_2020_2025.csv"])
    args = ap.parse_args()

    legs = OrderedDict()
    for key, (code, name) in LEGS.items():
        print(f"[抓取] {key:16s} {code} {name} ...", flush=True)
        daily = fetch_daily_returns(code)
        series = to_month_end(chain_link(daily))
        legs[key] = series
        months = list(series)
        print(f"         {len(daily)} 个交易日, {len(months)} 个月, "
              f"{months[0]} .. {months[-1]}")

    fgi = load_fgi_monthly(args.fgi)
    print(f"[FGI ] {len(fgi)} 个月, {list(fgi)[0]} .. {list(fgi)[-1]}")

    # 主序列：取所有腿与 FGI 共同覆盖的连续月份
    common = sorted(m for m in fgi if all(m in s for s in legs.values()))
    if not common:
        raise SystemExit("无共同覆盖月份")
    write_dataset(args.out, common, fgi, legs)

    # holdout：CNN 近一年 FGI，与主序列不连续，单独成块
    try:
        recent = fetch_cnn_recent_fgi()
    except Exception as exc:
        print(f"\n[警告] 抓取 CNN 近端 FGI 失败，跳过 holdout: {exc}")
        return
    hold = sorted(m for m in recent
                  if m > common[-1] and all(m in s for s in legs.values()))
    # 丢掉不完整的当月
    if hold and hold[-1] == max(max(s) for s in legs.values()):
        pass  # 末月为最新已收月，保留
    if len(hold) < 6:
        print(f"\n[警告] holdout 仅 {len(hold)} 个月，过短，跳过")
        return
    gap_from, gap_to = common[-1], hold[0]
    print(f"\n[holdout] 与主序列存在空档 {gap_from} -> {gap_to}"
          f"（CNN 仅提供约一年历史，无可达数据源补齐），故单独成块")
    write_dataset(args.holdout_out, hold, recent, legs)


if __name__ == "__main__":
    main()
