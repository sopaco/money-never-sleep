#!/usr/bin/env python3
"""买入护栏实验：情绪信号能否只用于否决/推迟「不合适的买入动作」。

与已证伪的择时实验的区别：
  - 择时: FGI -> 目标仓位（恐慌加仓、贪婪减仓），双向干预，已判定无显著贡献
  - 护栏: FGI -> 只允许把过热月份的买入顺延，绝不额外加仓、不减仓
  - A/B/C/E/F 各组总投入金额完全相同，差异仅来自「钱落在哪个时点」

三层检验：
  诊断一   「过热之后是否更差」：不重叠 20 日窗口 + 年块 bootstrap（2000 次）
  诊断一b  「过热之后尾部是否更厚」：重叠 60 日窗口，下跌概率与 5% 左尾
  诊断二   「只否决的护栏净效果」：定额 / 追高 / 事件型追高 / 多阈值护栏

复现：
  python3 .agents/skills/mns-backtest/data/guardrail_experiment.py
  依赖 long_daily.csv 与 hs300_daily.csv（先跑 mns backtest update-data）

结论与边界见 docs/guardrail-experiment-2026-11-08.md。
本脚本只做验证，不产生交易信号，不构成投资建议。
"""
import csv
import random
import statistics
from collections import defaultdict
from pathlib import Path

DATA = Path(__file__).resolve().parent
W = (0.45, 0.35, 0.20)
LEGS = ("us_stocks", "cn_stocks", "counter_cyclical")
CASH_ANNUAL = 0.013
TRADING_DAYS = 252
FEE = 0.0003
SEED = 20261108


def load(name):
    with open(DATA / name) as f:
        rows = [r for r in csv.DictReader(l for l in f if not l.startswith("#"))]
    out = []
    for r in rows:
        try:
            fgi = float(r["fgi"])
            px = [float(r[k]) for k in LEGS]
            if fgi < 0 or any(v <= 0 for v in px):
                continue
            out.append({"date": r["date"], "fgi": fgi, "px": px})
        except (ValueError, KeyError):
            continue
    return out


def portfolio_nav(rows):
    nav = [1.0]
    for i in range(1, len(rows)):
        r = sum(W[k] * (rows[i]["px"][k] / rows[i - 1]["px"][k] - 1.0) for k in range(3))
        nav.append(nav[-1] * (1.0 + r))
    return nav


def fwd_returns(nav, horizon):
    return [nav[i + horizon] / nav[i] - 1.0 if i + horizon < len(nav) else None
            for i in range(len(nav))]


def irr(cashflows, lo=-0.95, hi=1.0):
    """cashflows: [(t_in_years, signed_amount)]，求 NPV=0 的年化利率。"""
    def npv(r):
        return sum(a / (1 + r) ** t for t, a in cashflows)
    if npv(lo) * npv(hi) > 0:
        return float("nan")
    for _ in range(120):
        mid = (lo + hi) / 2
        if npv(lo) * npv(mid) <= 0:
            hi = mid
        else:
            lo = mid
    return (lo + hi) / 2


def hot_conditions(rows, nav):
    """三种『过热』定义，全部只用当日可知信息。"""
    r60 = [nav[i] / nav[max(0, i - 60)] - 1.0 if i >= 60 else 0.0
            for i in range(len(rows))]
    return {
        "H1 FGI>=75": lambda i: rows[i]["fgi"] >= 75,
        "H2 FGI>=70": lambda i: rows[i]["fgi"] >= 70,
        "H3 FGI>=65 且近60日涨幅>=8%": lambda i: rows[i]["fgi"] >= 65 and r60[i] >= 0.08,
        "H4 近60日涨幅>=12%": lambda i: r60[i] >= 0.12,
    }


def diagnose(rows, nav, label):
    print(f"\n{'=' * 94}")
    print(f"  诊断一 · {label}：『过热』之后是否真的更差（不重叠 20 日窗口）")
    print(f"{'=' * 94}")
    fwd20 = fwd_returns(nav, 20)
    fwd60 = fwd_returns(nav, 60)
    sample_idx = [i for i in range(0, len(rows) - 60, 20)]
    by_year = defaultdict(list)
    for i in sample_idx:
        by_year[rows[i]["date"][:4]].append(i)
    years = sorted(by_year)
    rng = random.Random(SEED)

    print(f"  {'条件':<30}{'n(热/其他)':>13}{'20日差':>11}{'60日差':>11}"
          f"{'20日差 95% CI':>26}{'判定':>12}")
    print("  " + "-" * 90)
    for name, cond in hot_conditions(rows, nav).items():
        hot20 = [fwd20[i] for i in sample_idx if cond(i) and fwd20[i] is not None]
        oth20 = [fwd20[i] for i in sample_idx if not cond(i) and fwd20[i] is not None]
        hot60 = [fwd60[i] for i in sample_idx if cond(i) and fwd60[i] is not None]
        oth60 = [fwd60[i] for i in sample_idx if not cond(i) and fwd60[i] is not None]
        if not hot20 or not oth20:
            print(f"  {name:<30} 样本不足")
            continue
        d20 = statistics.mean(hot20) - statistics.mean(oth20)
        d60 = (statistics.mean(hot60) - statistics.mean(oth60)) if hot60 and oth60 else float("nan")
        # 年块 bootstrap：每年整体有放回抽取，保留 regime 与窗口自相关
        boot = []
        for _ in range(2000):
            picks = [by_year[rng.choice(years)] for _ in years]
            h = [fwd20[i] for blk in picks for i in blk if cond(i) and fwd20[i] is not None]
            o = [fwd20[i] for blk in picks for i in blk if not cond(i) and fwd20[i] is not None]
            if len(h) >= 3 and len(o) >= 3:
                boot.append(statistics.mean(h) - statistics.mean(o))
        lo = sorted(boot)[int(len(boot) * 0.025)] if boot else float("nan")
        hi = sorted(boot)[int(len(boot) * 0.975)] if boot else float("nan")
        verdict = "显著更差" if hi < 0 else ("显著更好" if lo > 0 else "不显著")
        print(f"  {name:<30}{f'{len(hot20)}/{len(oth20)}':>13}{d20 * 100:>+10.3f}pp"
              f"{d60 * 100:>+10.3f}pp"
              f"{f'[{lo * 100:+.2f}, {hi * 100:+.2f}]':>26}{verdict:>12}")


def tail_diagnose(rows, nav, label):
    """护栏针对的是尾部风险而非均值：检验过热后下跌概率、左尾与窗口内回撤。"""
    print(f"\n{'=' * 94}")
    print(f"  诊断一b · {label}：过热之后的『尾部』是否更厚（重叠 60 日窗口，年块 bootstrap）")
    print(f"{'=' * 94}")
    fwd60 = fwd_returns(nav, 60)
    by_year = defaultdict(list)
    for i in range(len(rows) - 60):
        by_year[rows[i]["date"][:4]].append(i)
    years = sorted(by_year)
    rng = random.Random(SEED + 1)
    print(f"  {'条件':<30}{'n(热/其他)':>13}{'下跌概率差':>13}{'5%左尾差':>13}"
          f"{'下跌概率差 95% CI':>24}")
    print("  " + "-" * 90)
    for name, cond in hot_conditions(rows, nav).items():
        hot = [fwd60[i] for i in range(len(rows) - 60) if cond(i)]
        oth = [fwd60[i] for i in range(len(rows) - 60) if not cond(i)]
        if len(hot) < 10 or not oth:
            print(f"  {name:<30} 样本不足")
            continue
        p_hot = sum(v < 0 for v in hot) / len(hot)
        p_oth = sum(v < 0 for v in oth) / len(oth)
        q5_hot = sorted(hot)[int(len(hot) * 0.05)]
        q5_oth = sorted(oth)[int(len(oth) * 0.05)]
        boot = []
        for _ in range(2000):
            picks = [by_year[rng.choice(years)] for _ in years]
            h = [fwd60[i] for blk in picks for i in blk if cond(i)]
            o = [fwd60[i] for blk in picks for i in blk if not cond(i)]
            if len(h) >= 10 and o:
                boot.append(sum(v < 0 for v in h) / len(h)
                            - sum(v < 0 for v in o) / len(o))
        lo = sorted(boot)[int(len(boot) * 0.025)]
        hi = sorted(boot)[int(len(boot) * 0.975)]
        print(f"  {name:<30}{f'{len(hot)}/{len(oth)}':>13}"
              f"{(p_hot - p_oth) * 100:>+11.1f}pp{(q5_hot - q5_oth) * 100:>+12.2f}pp"
              f"{f'[{lo * 100:+.1f}, {hi * 100:+.1f}]pp':>24}"
              f"{'  ← 尾部显著更厚' if lo > 0 else ''}")


def monthly_indices(rows):
    out, seen = [], set()
    for i, r in enumerate(rows):
        key = r["date"][:7]
        if key not in seen:
            seen.add(key)
            out.append(i)
    return out


def invest(rows, nav, amounts):
    """按月初金额买入并持有；未投现金按 1.3% 年化计息；返回期末与真 IRR。"""
    idxs = monthly_indices(rows)
    amount_by_idx = {idxs[m]: a for m, a in enumerate(amounts)}
    daily_growth = (1 + CASH_ANNUAL) ** (1 / TRADING_DAYS)
    shares = cash = fees = 0.0
    equity, cashflows = [], []
    t0 = None
    for i, r in enumerate(rows):
        cash *= daily_growth
        if i in amount_by_idx:
            amt = amount_by_idx[i]
            if t0 is None:
                t0 = i
            if amt > 0:
                fee = amt * FEE
                shares += (amt - fee) / nav[i]
                fees += fee
                cashflows.append(((i - t0) / TRADING_DAYS, -amt))
            cash -= 0.0
        equity.append(cash + shares * nav[i])
    final = equity[-1]
    cashflows.append(((len(rows) - 1 - (t0 or 0)) / TRADING_DAYS, final))
    peak, dd = equity[0], 0.0
    for v in equity:
        peak = max(peak, v)
        dd = min(dd, v / peak - 1.0)
    spent = sum(amounts)
    return {"final": final, "spent": spent, "shares": shares, "fees": fees,
            "dd": dd, "avg_cost": spent / shares if shares else 0.0,
            "irr": irr(cashflows), "buys": sum(a > 0 for a in amounts)}


def chase_amounts(rows, nav, idxs, total, gain):
    heat = []
    for i in idxs:
        r60 = nav[i] / nav[max(0, i - 60)] - 1.0 if i >= 60 else 0.0
        heat.append(max(0.0, r60) * 3.0 + max(0.0, rows[i]["fgi"] - 50.0) / 50.0)
    raw = [1.0 + gain * h for h in heat]
    s = sum(raw)
    return [total * x / s for x in raw]


def event_amounts(rows, nav, idxs, total, cond, multiple=3.0):
    """事件型追高：仅在高热月份临时把当月投入放大到 multiple 倍，其余月份等比缩减。"""
    flags = [cond(i) for i in idxs]
    raw = [multiple if f else 1.0 for f in flags]
    s = sum(raw)
    return [total * x / s for x in raw], sum(flags)


def deferred(amounts, rows, idxs, threshold, cond=None):
    """护栏：过热月全额顺延到下一个不过热月；末尾补齐，保证总投入不变。"""
    out, pending = [], 0.0
    for m, amt in enumerate(amounts):
        i = idxs[m]
        hot = (rows[i]["fgi"] >= threshold) if cond is None else cond(i)
        if hot:
            pending += amt
            out.append(0.0)
        else:
            out.append(amt + pending)
            pending = 0.0
    if pending > 0:
        for m in range(len(out) - 1, -1, -1):
            if out[m] > 0:
                out[m] += pending
                break
    return out


def guardrail(rows, nav, label):
    idxs = monthly_indices(rows)
    n = len(idxs)
    total = 1000.0 * n
    print(f"\n{'=' * 94}")
    print(f"  诊断二 · {label}：护栏只做否决/顺延，三组总投入完全相同 ¥{total:,.0f}（{n} 个月）")
    print(f"{'=' * 94}")

    flat = [1000.0] * n
    scenarios = [("A 定额定期（理性基线）", flat)]

    for gain, tag in [(1.5, "温和"), (4.0, "极端")]:
        chase = chase_amounts(rows, nav, idxs, total, gain)
        scenarios.append((f"B{gain} 追高型基线（{tag}）", chase))
        for thr in (65, 70, 75, 80):
            scenarios.append((f"C 护栏 FGI>={thr}（追高{gain}）",
                              deferred(chase, rows, idxs, thr)))

    conds = hot_conditions(rows, nav)
    ev, nev = event_amounts(rows, nav, idxs, total, conds["H3 FGI>=65 且近60日涨幅>=8%"])
    scenarios.append((f"E 事件型追高（{nev} 次临时加仓）", ev))
    scenarios.append(("F 事件型 + 护栏 H3", deferred(ev, rows, idxs, 999, conds["H3 FGI>=65 且近60日涨幅>=8%"])))

    print(f"\n  {'方案':<32}{'期末资产':>12}{'IRR':>9}{'买入成本':>11}"
          f"{'MaxDD':>10}{'买入次数':>9}")
    print("  " + "-" * 90)
    res = {}
    for name, amounts in scenarios:
        r = invest(rows, nav, amounts)
        res[name] = r
        print(f"  {name:<32}{r['final']:>11,.0f}{r['irr'] * 100:>8.2f}%"
              f"{r['avg_cost']:>11.4f}{r['dd'] * 100:>9.2f}%{r['buys']:>9}")

    a = res["A 定额定期（理性基线）"]["final"]
    print(f"\n  {'对照':<32}{'相对理性基线':>14}{'相对各追高基线':>16}")
    print("  " + "-" * 66)
    for gain in (1.5, 4.0):
        b = res[f"B{gain} 追高型基线（{'温和' if gain == 1.5 else '极端'}）"]["final"]
        print(f"  {'B' + str(gain) + ' 行为缺陷本身的代价':<32}{b - a:>+13,.0f}{'—':>16}")
        for thr in (65, 70, 75, 80):
            key = f"C 护栏 FGI>={thr}（追高{gain}）"
            v = res[key]["final"]
            print(f"  {key:<32}{v - a:>+13,.0f}{v - b:>+16,.0f}")
    be = res[f"E 事件型追高（{nev} 次临时加仓）"]["final"]
    fe = res["F 事件型 + 护栏 H3"]["final"]
    print(f"  {'E 事件型追高':<32}{be - a:>+13,.0f}{'—':>16}")
    print(f"  {'F 事件型 + 护栏 H3':<32}{fe - a:>+13,.0f}{fe - be:>+16,.0f}")
    return res


def main():
    for name, label in [("long_daily.csv", "三资产组合 45/35/20"), ("hs300_daily.csv", "沪深300 对照")]:
        rows = load(name)
        nav = portfolio_nav(rows)
        diagnose(rows, nav, label)
        tail_diagnose(rows, nav, label)
        guardrail(rows, nav, label)
    print("\n注：『追高型/事件型基线』是人为设定的行为模型，非真实用户流水；")
    print("    结论是条件性的——取决于「人本来会犯多大的错」，该反事实无法从行情数据中测出。")
    print("    成本按场内 ETF 单边 0.03%，未建模 QDII 限购、T+n 到账与跟踪误差。")
    print("    年块 bootstrap seed=20261108，2000 次，可按此脚本复现。")


if __name__ == "__main__":
    main()
