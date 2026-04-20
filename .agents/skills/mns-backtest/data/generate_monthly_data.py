#!/usr/bin/env python3
"""
整合过去10年（2016-2025）的多资产月度数据：
- 纳斯达克综合指数 (^IXIC)
- 沪深300指数 (000300)
- 人民币黄金价格（上海黄金交易所AU9999，元/克）
- CNN恐贪指数（月末值）

数据来源：
- 纳斯达克：yfinance (雅虎财经)
- 沪深300：手动补充的历史数据 + yfinance
- 人民币黄金：yfinance (黄金ETF 518880.SS 作为代理)
- 恐贪指数：现有CSV文件聚合
"""

import yfinance as yf
import pandas as pd
from datetime import datetime
import os

DATA_DIR = os.path.dirname(os.path.abspath(__file__))

def fetch_nasdaq_monthly() -> pd.DataFrame:
    """获取纳斯达克综合指数月度数据"""
    print("获取纳斯达克指数...")
    ticker = yf.Ticker('^IXIC')
    df = ticker.history(start='2016-01-01', end='2025-05-01', interval='1d', auto_adjust=True)
    
    # 月末收盘价
    monthly = df['Close'].resample('ME').last()
    result = pd.DataFrame({
        'date': monthly.index.strftime('%Y-%m'),
        'nasdaq': monthly.values
    })
    print(f"  纳斯达克: {len(result)} 条")
    return result

def load_hs300_data() -> pd.DataFrame:
    """加载沪深300月度数据"""
    # 加载手动补充的历史数据
    hs300 = pd.read_csv(os.path.join(DATA_DIR, 'hs300_monthly.csv'))
    print(f"  沪深300: {len(hs300)} 条")
    return hs300

def fetch_gold_cny_monthly() -> pd.DataFrame:
    """
    获取人民币黄金月度数据
    使用518880.SS（华安黄金ETF）作为代理
    价格需要转换为元/克
    """
    print("获取人民币黄金数据...")
    ticker = yf.Ticker('518880.SS')
    df = ticker.history(start='2016-01-01', end='2025-05-01', interval='1d', auto_adjust=True)
    
    # 月末收盘价
    monthly = df['Close'].resample('ME').last()
    
    # 转换为元/克：ETF净值约等于金价/100，需要根据实际情况调整
    # 518880.SS 价格约为AU9999价格/100，实际金价 = ETF价格 * 100
    # 但ETF有折溢价，我们直接使用原始价格作为金价代理指标
    gold_prices = monthly.values
    
    result = pd.DataFrame({
        'date': monthly.index.strftime('%Y-%m'),
        'gold_cny': gold_prices
    })
    print(f"  人民币黄金: {len(result)} 条")
    return result

def load_fgi_monthly() -> pd.DataFrame:
    """加载并聚合恐贪指数月度数据"""
    # 加载 2016-2020 逐日数据
    fgi_2016_2020 = pd.read_csv(
        os.path.join(DATA_DIR, 'fgi_2016_2020.csv'),
        names=['date', 'fgi']
    )
    
    # 加载 2020-2025 数据
    fgi_2020_2025 = pd.read_csv(
        os.path.join(DATA_DIR, 'fgi_2020_2025.csv'),
        names=['date', 'fgi']
    )
    
    # 合并
    fgi_all = pd.concat([fgi_2016_2020, fgi_2020_2025], ignore_index=True)
    fgi_all['date'] = pd.to_datetime(fgi_all['date'])
    
    # 取月末值
    fgi_all['year_month'] = fgi_all['date'].dt.strftime('%Y-%m')
    monthly_fgi = fgi_all.sort_values('date').groupby('year_month').last().reset_index()
    
    result = monthly_fgi[['year_month', 'fgi']].rename(columns={'year_month': 'date'})
    print(f"  恐贪指数: {len(result)} 条")
    return result

def main():
    """主函数：整合所有数据"""
    print("=" * 60)
    print("生成过去10年月度多资产数据")
    print("=" * 60)
    
    # 获取各类数据
    nasdaq_df = fetch_nasdaq_monthly()
    hs300_df = load_hs300_data()
    gold_df = fetch_gold_cny_monthly()
    fgi_df = load_fgi_monthly()
    
    # 合并数据
    print("\n合并数据...")
    merged = fgi_df.merge(nasdaq_df, on='date', how='outer')
    merged = merged.merge(hs300_df, on='date', how='outer')
    merged = merged.merge(gold_df, on='date', how='outer')
    
    # 按日期排序
    merged = merged.sort_values('date').reset_index(drop=True)
    
    # 确保列顺序
    merged = merged[['date', 'fgi', 'nasdaq', 'hs300', 'gold_cny']]
    
    # 保存结果
    output_path = os.path.join(DATA_DIR, 'monthly_multi_asset.csv')
    merged.to_csv(output_path, index=False, float_format='%.2f')
    
    print(f"\n数据已保存到: {output_path}")
    print(f"总计 {len(merged)} 条月度数据")
    
    # 数据质量检查
    print("\n数据质量检查:")
    print(merged.isnull().sum().to_string())
    
    # 显示数据预览
    print("\n数据预览 (前10行):")
    print(merged.head(10).to_string())
    print("\n数据预览 (后10行):")
    print(merged.tail(10).to_string())
    
    # 计算收益率统计
    print("\n收益率统计 (2016-2025):")
    for col in ['nasdaq', 'hs300', 'gold_cny']:
        if merged[col].notna().sum() > 1:
            first_valid = merged[merged[col].notna()].iloc[0]
            last_valid = merged[merged[col].notna()].iloc[-1]
            total_return = (last_valid[col] / first_valid[col] - 1) * 100
            years = (pd.to_datetime(last_valid['date']) - pd.to_datetime(first_valid['date'])).days / 365.25
            annualized = ((last_valid[col] / first_valid[col]) ** (1/years) - 1) * 100
            print(f"  {col}: 总收益 {total_return:.1f}%, 年化 {annualized:.2f}%")
    
    return merged

if __name__ == '__main__':
    main()
