#!/usr/bin/env python3
"""
获取过去10年的月度金融数据：
- 纳斯达克综合指数 (^IXIC)
- 沪深300指数 (000300.SS)
- 人民币黄金 (AU9999.SGE 或黄金ETF 518880.SS)
- CNN恐贪指数（从现有数据整合）

数据源：
- yfinance（雅虎财经）
- 现有恐贪指数数据文件
"""

import yfinance as yf
import pandas as pd
from datetime import datetime, timedelta
import os

# 设置数据路径
DATA_DIR = os.path.dirname(os.path.abspath(__file__))
START_DATE = "2016-01-01"
END_DATE = "2025-04-30"

def fetch_index_data(symbol: str, name: str) -> pd.DataFrame:
    """
    从 yfinance 获取指数月度数据
    返回月末收盘价
    """
    print(f"正在获取 {name} ({symbol}) 数据...")
    
    ticker = yf.Ticker(symbol)
    
    # 获取历史数据
    df = ticker.history(
        start=START_DATE,
        end=END_DATE,
        interval="1d",
        auto_adjust=True
    )
    
    if df.empty:
        print(f"警告: {name} 数据为空")
        return pd.DataFrame()
    
    # 重采样到月度（取月末收盘价）
    monthly = df['Close'].resample('ME').last()
    monthly_df = pd.DataFrame({
        'date': monthly.index.strftime('%Y-%m'),
        name: monthly.values
    })
    
    print(f"  获取到 {len(monthly_df)} 条月度数据")
    return monthly_df

def load_fgi_data() -> pd.DataFrame:
    """
    加载现有恐贪指数数据并聚合为月度数据
    """
    # 加载 2016-2020 数据
    fgi_2016_2020 = pd.read_csv(
        os.path.join(DATA_DIR, 'fgi_2016_2020.csv'),
        names=['date', 'fgi']
    )
    
    # 加载 2020-2025 数据
    fgi_2020_2025 = pd.read_csv(
        os.path.join(DATA_DIR, 'fgi_2020_2025.csv'),
        names=['date', 'fgi']
    )
    
    # 合并数据
    fgi_all = pd.concat([fgi_2016_2020, fgi_2020_2025], ignore_index=True)
    
    # 转换日期格式
    fgi_all['date'] = pd.to_datetime(fgi_all['date'])
    
    # 提取年月
    fgi_all['year_month'] = fgi_all['date'].dt.strftime('%Y-%m')
    
    # 取每月最后一个数据点（月末恐贪指数）
    monthly_fgi = fgi_all.sort_values('date').groupby('year_month').last().reset_index()
    monthly_fgi = monthly_fgi[['year_month', 'fgi']].rename(columns={'year_month': 'date'})
    
    print(f"恐贪指数: {len(monthly_fgi)} 条月度数据")
    return monthly_fgi

def main():
    """主函数：获取并合并所有数据"""
    
    # 1. 获取纳斯达克综合指数
    nasdaq_df = fetch_index_data('^IXIC', 'nasdaq')
    
    # 2. 获取沪深300指数
    # 注：沪深300在雅虎财经的代码可能不稳定，尝试多个代码
    hs300_df = pd.DataFrame()
    for symbol in ['000300.SS', '000300.SH', 'HS300.SS']:
        hs300_df = fetch_index_data(symbol, 'hs300')
        if not hs300_df.empty:
            break
    
    if hs300_df.empty:
        print("警告: 无法获取沪深300数据，使用备用方案...")
        # 备用方案：使用 yfinance 的中国市场数据
        hs300_df = fetch_index_data('ASHR', 'hs300')  # 安硕沪深300 ETF
    
    # 3. 获取人民币黄金数据
    # 尝试多个代码：上海金交所AU9999、黄金ETF
    gold_df = pd.DataFrame()
    for symbol in ['AU9999.SGE', '518880.SS', 'GLD']:  # AU9999、黄金ETF、国际黄金ETF
        gold_df = fetch_index_data(symbol, 'gold_cny')
        if not gold_df.empty:
            break
    
    # 4. 加载恐贪指数数据
    fgi_df = load_fgi_data()
    
    # 5. 合并所有数据
    print("\n合并数据...")
    merged_df = fgi_df.copy()
    
    if not nasdaq_df.empty:
        merged_df = merged_df.merge(nasdaq_df, on='date', how='outer')
    
    if not hs300_df.empty:
        merged_df = merged_df.merge(hs300_df, on='date', how='outer')
    
    if not gold_df.empty:
        merged_df = merged_df.merge(gold_df, on='date', how='outer')
    
    # 按日期排序
    merged_df = merged_df.sort_values('date').reset_index(drop=True)
    
    # 重命名列
    merged_df.columns = ['date', 'fgi', 'nasdaq', 'hs300', 'gold_cny']
    
    # 保存合并数据
    output_path = os.path.join(DATA_DIR, 'monthly_multi_asset.csv')
    merged_df.to_csv(output_path, index=False)
    print(f"\n数据已保存到: {output_path}")
    print(f"总行数: {len(merged_df)}")
    
    # 显示数据预览
    print("\n数据预览:")
    print(merged_df.head(10).to_string())
    print("...")
    print(merged_df.tail(10).to_string())
    
    # 数据统计
    print("\n数据统计:")
    print(merged_df.describe().to_string())
    
    return merged_df

if __name__ == '__main__':
    main()
