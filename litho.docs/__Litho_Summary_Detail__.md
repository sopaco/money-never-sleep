# Project Analysis Summary Report (Full Version)

Generation Time: 2026-04-19 15:39:12 UTC

## Execution Timing Statistics

- **Total Execution Time**: 593.55 seconds
- **Preprocessing Phase**: 67.60 seconds (11.4%)
- **Research Phase**: 191.79 seconds (32.3%)
- **Document Generation Phase**: 334.16 seconds (56.3%)
- **Output Phase**: 0.00 seconds (0.0%)
- **Summary Generation Time**: 0.002 seconds

## Cache Performance Statistics and Savings

### Performance Metrics
- **Cache Hit Rate**: 0.0%
- **Total Operations**: 36
- **Cache Hits**: 0 times
- **Cache Misses**: 36 times
- **Cache Writes**: 37 times

### Savings
- **Inference Time Saved**: 0.0 seconds
- **Tokens Saved**: 0 input + 0 output = 0 total
- **Estimated Cost Savings**: $0.0000

## Core Research Data Summary

Complete content of four types of research materials according to Prompt template data integration rules:

### System Context Research Report
Provides core objectives, user roles, and system boundary information for the project.

```json
{
  "business_value": "为个人投资者提供一套基于市场情绪（CNN恐惧与贪婪指数）和资产表现的自动化逆向投资策略系统，通过CLI工具实现资产组合管理、智能买卖建议、风险预警与日报生成，提升投资决策的纪律性与数据驱动性，降低情绪化交易风险。",
  "confidence_score": 0.98,
  "external_systems": [
    {
      "description": "提供实时市场情绪指标（恐惧与贪婪指数），作为策略决策的核心外部数据源。",
      "interaction_type": "HTTP GET (REST API)",
      "name": "CNN Fear & Greed Index API"
    }
  ],
  "project_description": "mns（Market Neutral Strategist）是一个命令行工具，专为个人投资者设计，通过整合市场情绪数据、资产持仓信息与动态交易策略，实现自动化投资建议与组合管理。系统支持现金管理、资产买入/卖出、价格更新、 sentiment 分析、持仓评估与每日策略报告生成，所有操作基于可配置的规则引擎，强调风险控制与反向操作逻辑。",
  "project_name": "mns",
  "project_type": "CLITool",
  "system_boundary": {
    "excluded_components": [],
    "included_components": [
      "main.rs",
      "cli.rs",
      "config.rs",
      "db.rs",
      "models.rs",
      "report.rs",
      "sentiment.rs",
      "strategy.rs"
    ],
    "scope": "本系统为独立命令行工具，不包含Web界面、移动应用、后端服务或第三方交易平台接口。所有数据持久化使用本地SQLite数据库，所有外部交互仅限于获取CNN市场情绪数据。策略计算、报告生成与用户交互均在本地完成，无远程服务器依赖。"
  },
  "target_users": [
    {
      "description": "具备一定投资经验、追求长期价值、厌恶情绪化交易的个人投资者，希望借助数据驱动的规则系统辅助决策。",
      "name": "个人逆向投资者",
      "needs": [
        "根据市场情绪自动判断买入/卖出时机",
        "避免在亏损资产上过度加仓（防止‘接飞刀’）",
        "动态调整资产配置比例以实现风险对冲",
        "获取每日中文投资策略报告以复盘与执行",
        "保持投资纪律，减少主观判断干扰"
      ]
    }
  ]
}
```

### Domain Modules Research Report
Provides high-level domain division, module relationships, and core business process information.

```json
{
  "architecture_summary": "mns系统采用清晰的分层架构，以命令行入口为核心，通过模块化设计实现业务逻辑与基础设施的分离。核心业务逻辑由策略引擎、报告生成和资产模型驱动，基础设施层提供配置管理、数据持久化和外部数据获取能力。系统基于Rust构建，使用SQLite本地存储、TOML配置文件和HTTP客户端实现轻量级、无服务依赖的本地投资助手，强调高内聚、低耦合与可配置性，符合个人投资者对数据驱动、纪律性交易的需求。",
  "business_flows": [
    {
      "description": "根据当前持仓、市场情绪和配置规则，自动生成包含买卖建议、风险预警与资金分配预案的中文日报，帮助用户复盘与执行投资决策。",
      "entry_point": "用户执行 'mns report' 命令",
      "importance": 9.5,
      "involved_domains_count": 5,
      "name": "每日投资策略报告生成流程",
      "steps": [
        {
          "code_entry_point": "src/config.rs",
          "domain_module": "配置管理",
          "operation": "加载配置文件，获取资产分配比例、阈值和API端点",
          "step": 1,
          "sub_module": null
        },
        {
          "code_entry_point": "src/db.rs",
          "domain_module": "数据库",
          "operation": "查询当前现金余额、持仓记录和交易历史",
          "step": 2,
          "sub_module": null
        },
        {
          "code_entry_point": "src/sentiment.rs",
          "domain_module": "情感分析",
          "operation": "通过HTTP请求获取CNN恐惧与贪婪指数实时数据",
          "step": 3,
          "sub_module": null
        },
        {
          "code_entry_point": "src/strategy.rs",
          "domain_module": "策略引擎",
          "operation": "计算买入建议、卖出建议和风险预警，基于持仓、情绪和配置规则",
          "step": 4,
          "sub_module": null
        },
        {
          "code_entry_point": "src/report.rs",
          "domain_module": "报告生成",
          "operation": "整合所有数据，格式化为中文日报并保存至本地文件",
          "step": 5,
          "sub_module": null
        }
      ]
    },
    {
      "description": "处理用户发起的买入、卖出、加仓或价格更新操作，确保资产组合与现金余额的实时一致性，并记录完整交易历史。",
      "entry_point": "用户执行 'mns buy', 'mns sell', 'mns add' 或 'mns price' 命令",
      "importance": 9.0,
      "involved_domains_count": 4,
      "name": "资产交易与持仓更新流程",
      "steps": [
        {
          "code_entry_point": "src/cli.rs",
          "domain_module": "命令行接口",
          "operation": "解析用户输入的命令与参数，验证合法性",
          "step": 1,
          "sub_module": null
        },
        {
          "code_entry_point": "src/db.rs",
          "domain_module": "数据库",
          "operation": "启动事务，更新持仓数量、成本价、购买日期或现金余额",
          "step": 2,
          "sub_module": null
        },
        {
          "code_entry_point": "src/models.rs",
          "domain_module": "数据模型",
          "operation": "计算并验证交易后的资产价值、年化收益率与风险指标",
          "step": 3,
          "sub_module": null
        },
        {
          "code_entry_point": "src/config.rs",
          "domain_module": "配置管理",
          "operation": "校验交易是否违反配置规则（如负现金余额、类别限制）",
          "step": 4,
          "sub_module": null
        }
      ]
    },
    {
      "description": "首次运行时创建配置文件与数据库结构，后续支持动态配置修改与验证，确保系统始终处于合法运行状态。",
      "entry_point": "用户执行 'mns init' 或 'mns config' 命令",
      "importance": 8.0,
      "involved_domains_count": 3,
      "name": "系统初始化与配置管理流程",
      "steps": [
        {
          "code_entry_point": "src/cli.rs",
          "domain_module": "命令行接口",
          "operation": "识别 init 或 config 子命令，引导用户输入或修改参数",
          "step": 1,
          "sub_module": null
        },
        {
          "code_entry_point": "src/config.rs",
          "domain_module": "配置管理",
          "operation": "创建默认配置文件（config.toml），验证分配总和为100%等业务规则",
          "step": 2,
          "sub_module": null
        },
        {
          "code_entry_point": "src/db.rs",
          "domain_module": "数据库",
          "operation": "初始化SQLite数据库，创建持仓、交易、现金、情绪四张表结构",
          "step": 3,
          "sub_module": null
        }
      ]
    }
  ],
  "confidence_score": 0.98,
  "domain_modules": [
    {
      "code_paths": [
        "src/strategy.rs"
      ],
      "complexity": 9.0,
      "description": "实现基于市场情绪与资产表现的自动化逆向投资策略，包含买入、卖出与风险预警的智能决策逻辑，是系统的核心价值所在。",
      "domain_type": "Core Business Domain",
      "importance": 9.5,
      "name": "核心策略引擎",
      "sub_modules": [
        {
          "code_paths": [
            "src/strategy.rs"
          ],
          "description": "根据可用资金、资产分配比例与反向权重机制，计算最优买入组合，避免‘接飞刀’",
          "importance": 9.0,
          "key_functions": [
            "calculate_buy_suggestions"
          ],
          "name": "买入建议计算"
        },
        {
          "code_paths": [
            "src/strategy.rs"
          ],
          "description": "基于年化收益率、绝对收益与市场情绪动态调整卖出优先级，实现利润锁定",
          "importance": 8.5,
          "key_functions": [
            "calculate_sell_suggestions"
          ],
          "name": "卖出建议计算"
        },
        {
          "code_paths": [
            "src/strategy.rs"
          ],
          "description": "识别亏损超阈值的持仓，结合情绪等级提供分级预警建议",
          "importance": 8.0,
          "key_functions": [
            "check_risk_warnings"
          ],
          "name": "风险预警系统"
        }
      ]
    },
    {
      "code_paths": [
        "src/report.rs"
      ],
      "complexity": 7.5,
      "description": "将策略引擎输出、市场情绪与持仓数据整合为可读性强的中文日报，提升用户决策效率与执行纪律。",
      "domain_type": "Core Business Domain",
      "importance": 9.0,
      "name": "报告生成服务",
      "sub_modules": [
        {
          "code_paths": [
            "src/report.rs"
          ],
          "description": "按结构化模板组织市场情绪、持仓详情、买卖建议与风险提示",
          "importance": 8.5,
          "key_functions": [
            "generate_report_content"
          ],
          "name": "报告内容编排"
        },
        {
          "code_paths": [
            "src/report.rs"
          ],
          "description": "将生成的报告保存为带时间戳的本地文件，支持历史回溯",
          "importance": 7.0,
          "key_functions": [
            "save_report"
          ],
          "name": "报告持久化"
        }
      ]
    },
    {
      "code_paths": [
        "src/models.rs",
        "src/db.rs"
      ],
      "complexity": 8.0,
      "description": "定义核心金融实体的数据结构，并提供SQLite数据库的CRUD操作，保障数据一致性与持久性。",
      "domain_type": "Infrastructure Domain",
      "importance": 8.5,
      "name": "数据模型与持久化",
      "sub_modules": [
        {
          "code_paths": [
            "src/models.rs"
          ],
          "description": "定义Position、Transaction、FearGreedSnapshot等核心数据结构，包含业务计算逻辑（如年化收益率）",
          "importance": 8.0,
          "key_functions": [],
          "name": "金融数据模型"
        },
        {
          "code_paths": [
            "src/db.rs"
          ],
          "description": "管理SQLite数据库连接、表初始化、事务控制与原子操作（如买卖同时更新持仓与现金）",
          "importance": 8.5,
          "key_functions": [
            "init_db",
            "update_position",
            "add_transaction",
            "fetch_cash_balance"
          ],
          "name": "数据库操作"
        }
      ]
    },
    {
      "code_paths": [
        "src/config.rs"
      ],
      "complexity": 7.0,
      "description": "作为系统唯一配置源，统一管理用户自定义规则、资产分配、阈值与外部API设置，支持动态查询与持久化。",
      "domain_type": "Infrastructure Domain",
      "importance": 9.0,
      "name": "配置管理",
      "sub_modules": [
        {
          "code_paths": [
            "src/config.rs"
          ],
          "description": "从TOML文件加载配置，验证分配总和、阈值合理性等业务规则",
          "importance": 8.5,
          "key_functions": [
            "load_config",
            "validate_config"
          ],
          "name": "配置加载与验证"
        },
        {
          "code_paths": [
            "src/config.rs"
          ],
          "description": "将CNN恐惧与贪婪指数映射为情绪区域，并动态计算买入/卖出比例",
          "importance": 7.5,
          "key_functions": [
            "get_buy_ratio",
            "get_sell_ratio",
            "map_fear_greed_to_zone"
          ],
          "name": "情绪-策略映射"
        }
      ]
    },
    {
      "code_paths": [
        "src/sentiment.rs"
      ],
      "complexity": 6.0,
      "description": "封装对CNN恐惧与贪婪指数API的HTTP请求，屏蔽网络细节，提供稳定、可重用的情感数据服务。",
      "domain_type": "Infrastructure Domain",
      "importance": 7.5,
      "name": "外部数据获取",
      "sub_modules": [
        {
          "code_paths": [
            "src/sentiment.rs"
          ],
          "description": "发送带Headers的HTTP GET请求，解析JSON响应，封装错误上下文",
          "importance": 7.5,
          "key_functions": [
            "fetch_fear_greed"
          ],
          "name": "API调用服务"
        }
      ]
    },
    {
      "code_paths": [
        "src/cli.rs"
      ],
      "complexity": 5.0,
      "description": "作为用户与系统交互的唯一入口，负责命令解析、参数校验与功能路由，不包含业务逻辑。",
      "domain_type": "Tool Support Domain",
      "importance": 8.0,
      "name": "命令行接口",
      "sub_modules": [
        {
          "code_paths": [
            "src/cli.rs"
          ],
          "description": "使用clap解析子命令（buy, sell, report, init等）与参数",
          "importance": 8.0,
          "key_functions": [],
          "name": "命令解析器"
        },
        {
          "code_paths": [
            "src/cli.rs"
          ],
          "description": "将解析后的命令委托至对应服务模块（策略、数据库、报告等）",
          "importance": 7.5,
          "key_functions": [],
          "name": "功能路由"
        }
      ]
    },
    {
      "code_paths": [
        "src/main.rs"
      ],
      "complexity": 6.0,
      "description": "程序启动点，协调各模块初始化与执行，负责主流程控制与用户反馈。",
      "domain_type": "Tool Support Domain",
      "importance": 8.5,
      "name": "系统入口",
      "sub_modules": [
        {
          "code_paths": [
            "src/main.rs"
          ],
          "description": "初始化配置、数据库、CLI，根据用户命令调用对应处理函数",
          "importance": 8.5,
          "key_functions": [
            "main",
            "handle_command"
          ],
          "name": "主流程协调"
        }
      ]
    }
  ],
  "domain_relations": [
    {
      "description": "主入口调用CLI模块解析用户命令，是系统启动与交互的首要依赖。",
      "from_domain": "系统入口",
      "relation_type": "Service Call",
      "strength": 9.0,
      "to_domain": "命令行接口"
    },
    {
      "description": "主入口在初始化阶段必须加载配置，为所有后续模块提供基础参数。",
      "from_domain": "系统入口",
      "relation_type": "Service Call",
      "strength": 9.0,
      "to_domain": "配置管理"
    },
    {
      "description": "主入口初始化数据库连接，确保数据持久化能力就绪。",
      "from_domain": "系统入口",
      "relation_type": "Service Call",
      "strength": 8.5,
      "to_domain": "数据库"
    },
    {
      "description": "CLI命令（如config、init）直接依赖配置模块进行参数读写与验证。",
      "from_domain": "命令行接口",
      "relation_type": "Configuration Dependency",
      "strength": 8.0,
      "to_domain": "配置管理"
    },
    {
      "description": "所有资产操作（buy/sell/add/price）均通过CLI路由至数据库进行持久化。",
      "from_domain": "命令行接口",
      "relation_type": "Service Call",
      "strength": 8.5,
      "to_domain": "数据库"
    },
    {
      "description": "sentiment命令直接触发外部情感数据获取。",
      "from_domain": "命令行接口",
      "relation_type": "Service Call",
      "strength": 7.0,
      "to_domain": "外部数据获取"
    },
    {
      "description": "report命令触发策略引擎计算买卖建议与风险预警。",
      "from_domain": "命令行接口",
      "relation_type": "Service Call",
      "strength": 8.0,
      "to_domain": "策略引擎"
    },
    {
      "description": "report命令最终调用报告生成模块输出结果。",
      "from_domain": "命令行接口",
      "relation_type": "Service Call",
      "strength": 8.5,
      "to_domain": "报告生成"
    },
    {
      "description": "报告内容需引用配置中的阈值、分配比例与情绪映射规则。",
      "from_domain": "报告生成",
      "relation_type": "Configuration Dependency",
      "strength": 8.0,
      "to_domain": "配置管理"
    },
    {
      "description": "报告需读取持仓、现金与交易历史等持久化数据。",
      "from_domain": "报告生成",
      "relation_type": "Data Dependency",
      "strength": 8.5,
      "to_domain": "数据库"
    },
    {
      "description": "报告需整合实时CNN情感指数作为分析依据。",
      "from_domain": "报告生成",
      "relation_type": "Data Dependency",
      "strength": 8.0,
      "to_domain": "外部数据获取"
    },
    {
      "description": "报告的核心内容（买卖建议、风险预警）完全依赖策略引擎的输出结果。",
      "from_domain": "报告生成",
      "relation_type": "Data Dependency",
      "strength": 9.0,
      "to_domain": "策略引擎"
    },
    {
      "description": "策略的买入/卖出比例、阈值、反向权重均来自配置模块的动态参数。",
      "from_domain": "策略引擎",
      "relation_type": "Configuration Dependency",
      "strength": 9.0,
      "to_domain": "配置管理"
    },
    {
      "description": "策略引擎必须读取当前持仓、成本价与现金余额以计算建议。",
      "from_domain": "策略引擎",
      "relation_type": "Data Dependency",
      "strength": 9.0,
      "to_domain": "数据库"
    },
    {
      "description": "策略决策依赖实时市场情绪数据作为输入变量。",
      "from_domain": "策略引擎",
      "relation_type": "Data Dependency",
      "strength": 8.5,
      "to_domain": "外部数据获取"
    },
    {
      "description": "策略引擎操作的数据结构（Position等）由数据模型模块定义。",
      "from_domain": "策略引擎",
      "relation_type": "Data Dependency",
      "strength": 8.0,
      "to_domain": "数据模型与持久化"
    },
    {
      "description": "API返回的JSON被反序列化为FearGreedResponse模型，供后续使用。",
      "from_domain": "外部数据获取",
      "relation_type": "Data Dependency",
      "strength": 7.5,
      "to_domain": "数据模型与持久化"
    },
    {
      "description": "数据库操作直接依赖数据模型结构进行序列化与反序列化。",
      "from_domain": "数据库",
      "relation_type": "Data Dependency",
      "strength": 9.0,
      "to_domain": "数据模型与持久化"
    }
  ]
}
```

### Workflow Research Report
Contains static analysis results of the codebase and business process analysis.

```json
"```markdown\n# System Workflow Analysis\n\n## 1. Main Workflow\n- **Workflow Name**: 每日投资策略报告生成流程\n- **Description**: 该流程是系统的核心价值输出路径，旨在为个人投资者提供一份结构化、数据驱动的中文投资日报。流程以实时市场情绪（CNN恐惧与贪婪指数）为触发信号，整合本地持仓数据、现金余额与用户自定义策略规则，通过策略引擎智能计算买入/卖出建议与风险预警，最终生成可读性强、具备执行指导意义的中文报告并持久化保存。整个流程实现“数据采集→智能分析→决策输出→结果沉淀”的闭环，帮助用户克服情绪干扰，执行纪律性逆向投资。\n- **Flow Diagram**:\n```mermaid\ngraph TD\n    A[用户执行 'mns report' 命令] --> B[加载配置文件：分配比例、阈值、API端点]\n    B --> C[查询数据库：获取现金余额、持仓记录、交易历史]\n    C --> D[调用CNN API：获取实时恐惧与贪婪指数]\n    D --> E[策略引擎计算：买入建议、卖出建议、风险预警]\n    E --> F[报告生成服务：整合所有数据，格式化为中文日报]\n    F --> G[保存报告：生成带时间戳的本地文件（如 report_2025-04-05.md）]\n    G --> H[完成：用户可查阅历史报告复盘决策]\n```\n- **Key Steps**:\n  1. **配置加载**：从 `.mns/config.toml` 加载资产分配比例（如美股/中股/反周期）、年化收益阈值、最小持有天数、情绪映射规则等核心策略参数，确保后续计算依据一致。\n  2. **数据查询**：从SQLite数据库中读取当前现金余额、所有持仓资产的成本价、当前价、购买日期及历史交易记录，构建完整的资产快照。\n  3. **情感数据获取**：通过HTTP请求调用CNN Fear & Greed Index API，获取最新市场情绪评分（0–100），并验证响应有效性。\n  4. **策略计算**：策略引擎根据持仓、现金、情绪评分及配置规则，分别计算：\n     - **买入建议**：基于可用资金与反向权重（亏损越深优先买入），排除亏损≥30%的“接飞刀”资产。\n     - **卖出建议**：优先卖出年化收益高且满足最低持有天数的资产，动态调整卖出比例（情绪越贪婪，卖出倾向越强）。\n     - **风险预警**：识别亏损>20%的持仓，结合情绪等级（恐惧/中性/贪婪）给出“考虑加仓”“审视基本面”或“紧急复盘”分级建议。\n  5. **报告生成**：将上述所有分析结果整合为结构化中文文本，使用ASCII表格清晰展示市场情绪、持仓详情、买卖清单与风险提示，并包含资金分配预案。\n  6. **报告持久化**：将生成的报告保存至本地 `reports/` 目录下，文件名包含时间戳（如 `report_2025-04-05.md`），支持历史回溯与审计。\n\n## 2. Other Important Workflows\n\n### 2.1 资产交易与持仓更新流程\n- **Description**: 该流程处理用户发起的所有资产操作指令（买入、卖出、加仓、价格更新），确保账户状态（现金余额、持仓数量、成本价）的实时一致性与事务完整性。所有操作均通过原子数据库事务执行，避免数据不一致，并自动记录交易日志，为策略分析与报告生成提供可靠数据源。\n- **Flow Diagram**:\n```mermaid\ngraph TD\n    A[用户执行 'mns buy/sell/add/price' 命令] --> B[CLI解析命令与参数：验证资产代码、数量、价格合法性]\n    B --> C[启动数据库事务：准备更新持仓与现金]\n    C --> D[更新持仓：修改数量、成本价（加权平均）、购买日期；或更新当前价格]\n    D --> E[更新现金余额：买方扣款，卖方入账]\n    E --> F[记录交易：写入交易历史表，含时间、类型、金额、资产]\n    F --> G[校验配置规则：防止负现金、类别超限等]\n    G --> H[提交事务：确保所有变更原子生效]\n    H --> I[返回成功提示或错误信息]\n```\n\n### 2.2 系统初始化与配置管理流程\n- **Description**: 该流程在首次运行或用户主动配置时，建立系统运行所需的基础设施与规则框架。包括创建默认配置文件、初始化本地数据库结构、验证配置项的业务合理性（如分配总和=100%），确保系统在合法、可预测的状态下运行，是后续所有业务流程的前提条件。\n- **Flow Diagram**:\n```mermaid\ngraph TD\n    A[用户执行 'mns init' 或 'mns config' 命令] --> B[CLI识别初始化/配置指令]\n    B --> C{是否首次运行？}\n    C -- 是 --> D[创建默认config.toml：设置标准分配比例与阈值]\n    C -- 否 --> E[打开现有config.toml供编辑]\n    D --> F[验证配置：分配总和=100%，阈值合理，API端点有效]\n    E --> F\n    F --> G[初始化SQLite数据库：创建cash、positions、transactions、fear_greed四张表]\n    G --> H[写入默认表结构与约束（如NOT NULL, UNIQUE, DEFAULT）]\n    H --> I[保存配置文件与数据库结构]\n    I --> J[提示用户：系统已初始化，可开始配置或使用]\n```\n\n## 3. Workflow Insights\n- **Key observations about the system's operational patterns**:\n  1. **数据驱动决策闭环**：系统所有核心流程（报告、交易、策略）均以“配置规则”为基准，以“数据库状态”为依据，以“外部情绪”为动态变量，形成高度结构化的决策链，显著降低主观干扰。\n  2. **事务性与一致性优先**：所有资产变动（buy/sell）均通过数据库事务原子化处理，确保现金与持仓同步更新，避免“账实不符”风险，体现金融级数据可靠性设计。\n  3. **情绪作为核心变量**：CNN恐惧与贪婪指数不仅是报告的展示内容，更是策略引擎的输入变量，直接影响买入权重与卖出比例，使系统具备“市场情绪感知”能力，是逆向投资逻辑的核心体现。\n  4. **报告是最终交付物**：无论用户执行何种命令（buy/sell），最终都可能被纳入报告生成流程的输入数据，说明“日报”是系统价值的集中体现，其他操作均服务于报告的准确性与丰富性。\n\n- **Potential optimization opportunities**:\n  1. **缓存机制引入**：CNN API调用频率较高，可增加本地缓存（如缓存1小时内的情绪数据），减少重复网络请求，提升响应速度并降低API调用配额压力。\n  2. **配置热更新支持**：当前配置需重启或重新执行`config`命令才生效，可增加`config reload`子命令或监听配置文件变更，实现运行时动态调整策略参数。\n  3. **报告模板可配置化**：当前报告格式为硬编码，未来可支持用户自定义模板（如Markdown模板文件），增强个性化与可扩展性。\n  4. **异步报告生成**：对于大数据量持仓，报告生成耗时可能较长，可考虑将报告生成任务放入后台异步执行，并返回“报告生成中”提示，提升CLI响应体验。\n\n- **Dependencies between workflows**:\n  1. **报告流程依赖所有其他流程**：每日报告的生成完全依赖于“资产交易流程”提供的最新持仓与现金数据、“配置管理流程”提供的规则参数、“外部数据获取”提供的市场情绪，是系统所有子流程的“聚合出口”。\n  2. **交易流程依赖配置与数据库**：任何交易操作均需校验配置规则（如禁止负现金），并依赖数据库完成原子更新，是系统“状态变更”的唯一入口。\n  3. **初始化流程是前提依赖**：若未执行`init`或`config`，则`report`、`buy`、`sell`等命令均无法正常运行，说明系统具有严格的“先配置后使用”约束，符合工具类软件的健壮性设计原则。\n  4. **策略引擎是核心枢纽**：策略引擎同时依赖配置（规则）、数据库（持仓）、外部API（情绪）三类数据源，是连接基础设施层与业务价值层的“大脑”，其健壮性直接决定系统整体质量。\n```"
```

### Code Insights Data
Code analysis results from preprocessing phase, including definitions of functions, classes, and modules.

```json
[
  {
    "code_dossier": {
      "code_purpose": "entry",
      "description": null,
      "file_path": "src\\main.rs",
      "functions": [],
      "importance_score": 1.0,
      "interfaces": [
        "main",
        "cmd_init",
        "cmd_config",
        "cmd_cash",
        "cmd_cash_set",
        "cmd_cash_add",
        "cmd_portfolio",
        "cmd_add",
        "cmd_buy",
        "cmd_sell",
        "cmd_price",
        "cmd_sentiment",
        "cmd_report",
        "cmd_history"
      ],
      "name": "main.rs",
      "source_summary": "mod cli;\r\nmod config;\r\nmod db;\r\nmod models;\r\nmod report;\r\nmod sentiment;\r\nmod strategy;\r\n\r\nuse anyhow::Result;\r\nuse clap::Parser;\r\nuse cli::{CashAction, Commands};\r\nuse comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Cell, Color, Table};\r\nuse config::AppConfig;\r\n\r\n#[tokio::main]\r\nasync fn main() -> Result<()> {\r\n    let cli = cli::Cli::parse();\r\n\r\n    match cli.command {\r\n        Commands::Init => cmd_init()?,\r\n        Commands::Config { key, value } => cmd_config(key, value)?,\r\n        Commands::Cash { action } => match action {\r\n            None => cmd_cash()?,\r\n            Some(CashAction::Set { amount }) => cmd_cash_set(amount)?,\r\n            Some(CashAction::Add { amount }) => cmd_cash_add(amount)?,\r\n        },\r\n        Commands::Portfolio => cmd_portfolio()?,\r\n        Commands::Add { code, name, category } => cmd_add(&code, &name, &category)?,\r\n        Commands::Buy { code, shares, price } => cmd_buy(&code, shares, price)?,\r\n        Commands::Sell { code, shares, price } => cmd_sell(&code, shares, price)?,\r\n        Commands::Price { code, price } => cmd_price(&code, price)?,\r\n        Commands::Sentiment => cmd_sentiment().await?,\r\n        Commands::Report => cmd_report().await?,\r\n        Commands::History { limit } => cmd_history(limit)?,\r\n    }\r\n\r\n    Ok(())\r\n}\r\n\r\nfn cmd_init() -> Result<()> {\r\n    let config = AppConfig::default_config();\r\n    config.save()?;\r\n\r\n    let db = db::Database::open()?;\r\n    drop(db);\r\n\r\n    // 创建报告输出目录\r\n    let report_dir = &config.settings.report_output_dir;\r\n    std::fs::create_dir_all(report_dir)?;\r\n\r\n    println!(\"✓ 初始化完成\");\r\n    println!(\"  配置文件: {}\", AppConfig::config_path()?.display());\r\n    println!(\"  数据库:   {}\", AppConfig::db_path()?.display());\r\n    println!(\"  报告目录: {}\", report_dir);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_config(key: Option<String>, value: Option<String>) -> Result<()> {\r\n    let mut config = AppConfig::load()?;\r\n\r\n    match (key, value) {\r\n        (None, None) => {\r\n            // 显示全部配置\r\n            let content = toml::to_string_pretty(&config)?;\r\n            println!(\"{}\", content);\r\n        }\r\n        (Some(k), None) => {\r\n            // 显示某个配置项\r\n            match config.get_value(&k) {\r\n                Some(v) => println!(\"{} = {}\", k, v),\r\n                None => anyhow::bail!(\"未知的配置项: {}\", k),\r\n            }\r\n        }\r\n        (Some(k), Some(v)) => {\r\n            // 修改配置项\r\n            config.set_value(&k, &v)?;\r\n            config.save()?;\r\n            println!(\"✓ {} = {}\", k, v);\r\n        }\r\n        (None, Some(_)) => unreachable!(),\r\n    }\r\n    Ok(())\r\n}\r\n\r\nfn cmd_cash() -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    let balance = db.get_cash_balance()?;\r\n    println!(\"现金余额: ¥{:.2}\", balance);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_cash_set(amount: f64) -> Result<()> {\r\n    if amount < 0.0 {\r\n        anyhow::bail!(\"现金余额不能为负数: {}\", amount);\r\n    }\r\n    let db = db::Database::open()?;\r\n    db.set_cash_balance(amount)?;\r\n    println!(\"✓ 现金余额已设置为: ¥{:.2}\", amount);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_cash_add(amount: f64) -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    let new_balance = db.add_cash(amount)?;\r\n    println!(\"✓ 已增加 ¥{:.2}，当前余额: ¥{:.2}\", amount, new_balance);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_portfolio() -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    let config = AppConfig::load()?;\r\n    let positions = db.list_positions()?;\r\n    let cash = db.get_cash_balance()?;\r\n\r\n    if positions.is_empty() {\r\n        println!(\"暂无持仓，使用 'mns add <code> <name> <category>' 添加资产\");\r\n        return Ok(());\r\n    }\r\n\r\n    let today = chrono::Local::now().date_naive();\r\n    let min_days = config.settings.min_holding_days;\r\n    let mut table = Table::new();\r\n    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);\r\n    table.set_header(vec![\r\n        Cell::new(\"代码\"),\r\n        Cell::new(\"名称\"),\r\n        Cell::new(\"类别\"),\r\n        Cell::new(\"份额\"),\r\n        Cell::new(\"成本价\"),\r\n        Cell::new(\"现价\"),\r\n        Cell::new(\"市值\"),\r\n        Cell::new(\"年化收益\"),\r\n        Cell::new(\"绝对收益\"),\r\n    ]);\r\n\r\n    let mut total_mv = 0.0;\r\n    for pos in &positions {\r\n        let mv = pos.market_value_or_cost();\r\n        total_mv += mv;\r\n        let ann = pos.annualized_return_with_min_days(&today, min_days);\r\n        let ann_str = match ann {\r\n            Some(r) => format!(\"{:+.1}%\", r * 100.0),\r\n            None => \"N/A\".to_string(),\r\n        };\r\n        let abs_str = match pos.absolute_return() {\r\n            Some(r) => format!(\"{:+.1}%\", r * 100.0),\r\n            None => \"N/A\".to_string(),\r\n        };\r\n        let price_str = match pos.current_price {\r\n            Some(p) => format!(\"{:.2}\", p),\r\n            None => \"-\".to_string(),\r\n        };\r\n        let category_cn = match pos.category.as_str() {\r\n            \"us_stocks\" => \"美股\",\r\n            \"cn_stocks\" => \"A股\",\r\n            \"counter_cyclical\" => \"逆周期\",\r\n            _ => &pos.category,\r\n        };\r\n        let mut ann_cell = Cell::new(&ann_str);\r\n        if let Some(r) = ann {\r\n            if r * 100.0 >= config.settings.annualized_target_high {\r\n                ann_cell = ann_cell.fg(Color::Green);\r\n            } else if r < 0.0 {\r\n                ann_cell = ann_cell.fg(Color::Red);\r\n            }\r\n        }\r\n        table.add_row(vec![\r\n            Cell::new(&pos.asset_code),\r\n            Cell::new(&pos.asset_name),\r\n            Cell::new(category_cn),\r\n            Cell::new(format!(\"{:.2}\", pos.shares)),\r\n            Cell::new(format!(\"{:.2}\", pos.cost_price)),\r\n            Cell::new(price_str),\r\n            Cell::new(format!(\"¥{:.2}\", mv)),\r\n            ann_cell,\r\n            Cell::new(&abs_str),\r\n        ]);\r\n    }\r\n\r\n    println!(\"{}\", table);\r\n    println!(\"\\n现金余额: ¥{:.2}\", cash);\r\n    println!(\"持仓市值: ¥{:.2}\", total_mv);\r\n    println!(\"总资产:   ¥{:.2}\", cash + total_mv);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_add(code: &str, name: &str, category: &str) -> Result<()> {\r\n    let valid_categories = [\"us_stocks\", \"cn_stocks\", \"counter_cyclical\"];\r\n    if !valid_categories.contains(&category) {\r\n        anyhow::bail!(\r\n            \"无效类别 '{}'，可选: {}\",\r\n            category,\r\n            valid_categories.join(\", \")\r\n        );\r\n    }\r\n    let db = db::Database::open()?;\r\n    db.add_position(code, name, category)?;\r\n    println!(\"✓ 已添加资产: {} ({}) [{}]\", code, name, category);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_buy(code: &str, shares: f64, price: f64) -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    let amount = shares * price;\r\n    db.buy_position(code, shares, price)?;\r\n    println!(\"✓ 买入 {} {:.2} 份 @ ¥{:.2}，合计 ¥{:.2}\", code, shares, price, amount);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_sell(code: &str, shares: f64, price: f64) -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    let amount = shares * price;\r\n    db.sell_position(code, shares, price)?;\r\n    println!(\"✓ 卖出 {} {:.2} 份 @ ¥{:.2}，合计 ¥{:.2}\", code, shares, price, amount);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_price(code: &str, price: Option<f64>) -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    match price {\r\n        Some(p) => {\r\n            db.update_price(code, p)?;\r\n            println!(\"✓ {} 当前价格已更新为 ¥{:.2}\", code, p);\r\n        }\r\n        None => {\r\n            let pos = db.get_position(code)?;\r\n            match pos {\r\n                Some(p) => {\r\n                    let cur = match p.current_price {\r\n                        Some(v) => format!(\"¥{:.2}\", v),\r\n                        None => \"未设置\".to_string(),\r\n                    };\r\n                    println!(\"{} ({}) 当前价格: {}\", p.asset_code, p.asset_name, cur);\r\n                }\r\n                None => anyhow::bail!(\"未找到资产: {}\", code),\r\n            }\r\n        }\r\n    }\r\n    Ok(())\r\n}\r\n\r\nasync fn cmd_sentiment() -> Result<()> {\r\n    let config = AppConfig::load()?;\r\n    println!(\"正在获取 CNN 恐贪指数...\");\r\n    let data = sentiment::fetch_fear_greed(&config).await?;\r\n\r\n    let zone = config.sentiment_zone(data.fear_and_greed.score);\r\n    println!(\"CNN 恐贪指数: {:.2} ({})\", data.fear_and_greed.score, zone);\r\n    if let Some(pc) = data.fear_and_greed.previous_close {\r\n        println!(\"前日收盘: {:.2}\", pc);\r\n    }\r\n    if let Some(pw) = data.fear_and_greed.previous_1_week {\r\n        println!(\"周环比: {:.2} → {:.2}\", pw, data.fear_and_greed.score);\r\n    }\r\n    if let Some(pm) = data.fear_and_greed.previous_1_month {\r\n        println!(\"月环比: {:.2} → {:.2}\", pm, data.fear_and_greed.score);\r\n    }\r\n    if let Some(py) = data.fear_and_greed.previous_1_year {\r\n        println!(\"年同比: {:.2} → {:.2}\", py, data.fear_and_greed.score);\r\n    }\r\n\r\n    // 保存快照\r\n    let db = db::Database::open()?;\r\n    db.save_fear_greed_snapshot(\r\n        data.fear_and_greed.score,\r\n        zone,\r\n        data.fear_and_greed.previous_close,\r\n        data.fear_and_greed.previous_1_week,\r\n        data.fear_and_greed.previous_1_month,\r\n        data.fear_and_greed.previous_1_year,\r\n    )?;\r\n\r\n    Ok(())\r\n}\r\n\r\nasync fn cmd_report() -> Result<()> {\r\n    let config = AppConfig::load()?;\r\n    let db = db::Database::open()?;\r\n\r\n    println!(\"正在获取 CNN 恐贪指数...\");\r\n    let data = sentiment::fetch_fear_greed(&config).await?;\r\n    let score = data.fear_and_greed.score;\r\n    let rating = config.sentiment_zone(score);\r\n\r\n    // 保存快照\r\n    db.save_fear_greed_snapshot(\r\n        score,\r\n        rating,\r\n        data.fear_and_greed.previous_close,\r\n        data.fear_and_greed.previous_1_week,\r\n        data.fear_and_greed.previous_1_month,\r\n        data.fear_and_greed.previous_1_year,\r\n    )?;\r\n\r\n    let cash = db.get_cash_balance()?;\r\n    let positions = db.list_positions()?;\r\n\r\n    // 策略计算（先算风险警告，再算买入建议以实现联动）\r\n    let sell_suggestions = strategy::calculate_sell_suggestions(&config, score, &positions);\r\n    let risk_warnings = strategy::check_risk_warnings(&config, score, &positions);\r\n    let buy_suggestion = strategy::calculate_buy_suggestions(&config, score, cash, &positions, &sell_suggestions, &risk_warnings);\r\n\r\n    // 生成报告\r\n    let report = report::generate_report(\r\n        &config,\r\n        score,\r\n        rating,\r\n        data.fear_and_greed.previous_close,\r\n        data.fear_and_greed.previous_1_week,\r\n        data.fear_and_greed.previous_1_month,\r\n        data.fear_and_greed.previous_1_year,\r\n        cash,\r\n        &positions,\r\n        &buy_suggestion,\r\n        &sell_suggestions,\r\n        &risk_warnings,\r\n    )?;\r\n\r\n    let filepath = report::save_report(&config, &report)?;\r\n    println!(\"{}\", report);\r\n    println!(\"\\n报告已保存至: {}\", filepath);\r\n    Ok(())\r\n}\r\n\r\nfn cmd_history(limit: i64) -> Result<()> {\r\n    let db = db::Database::open()?;\r\n    let txs = db.list_transactions(limit)?;\r\n\r\n    if txs.is_empty() {\r\n        println!(\"暂无交易记录\");\r\n        return Ok(());\r\n    }\r\n\r\n    let mut table = Table::new();\r\n    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);\r\n    table.set_header(vec![\"日期\", \"类型\", \"代码\", \"份额\", \"价格\", \"金额\"]);\r\n\r\n    for tx in &txs {\r\n        let type_label = if tx.tx_type == \"buy\" { \"买入\" } else { \"卖出\" };\r\n        table.add_row(vec![\r\n            Cell::new(&tx.tx_date),\r\n            Cell::new(type_label),\r\n            Cell::new(&tx.asset_code),\r\n            Cell::new(format!(\"{:.2}\", tx.shares)),\r\n            Cell::new(format!(\"{:.2}\", tx.price)),\r\n            Cell::new(format!(\"¥{:.2}\", tx.amount)),\r\n        ]);\r\n    }\r\n\r\n    println!(\"{}\", table);\r\n    Ok(())\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 26.0,
      "lines_of_code": 352,
      "number_of_classes": 0,
      "number_of_functions": 17
    },
    "dependencies": [
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 1,
        "name": "cli",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 2,
        "name": "config",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 3,
        "name": "db",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 4,
        "name": "models",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 5,
        "name": "report",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 6,
        "name": "sentiment",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "mod",
        "is_external": false,
        "line_number": 7,
        "name": "strategy",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 9,
        "name": "Result",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 10,
        "name": "Parser",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 11,
        "name": "CashAction",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 12,
        "name": "modifiers::UTF8_ROUND_CORNERS",
        "path": "src\\main.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 13,
        "name": "AppConfig",
        "path": "src\\main.rs",
        "version": null
      }
    ],
    "detailed_description": "The main.rs component serves as the entry point for the 'mns' (Market Neutral Strategist) command-line application, orchestrating user commands and delegating execution to modular subsystems. It parses CLI arguments via clap, routes each command to a corresponding handler function, and manages state through configuration (AppConfig) and database (Database) dependencies. The component handles core financial operations including cash balance management, asset position tracking (buy/sell/add), price updates, sentiment analysis (CNN Fear & Greed Index), portfolio valuation with annualized return calculations, transaction history display, and automated strategy report generation. Business logic includes risk-aware trading rules: buy/sell decisions are dynamically influenced by sentiment scores and portfolio performance metrics (e.g., annualized return vs. target thresholds), with portfolio valuation incorporating min_holding_days and contrarian weighting. The report generation workflow integrates real-time sentiment data, current portfolio state, and strategy rules to produce actionable investment guidance. All operations are transactional and validated (e.g., negative cash balance prevention, category validation), with user feedback provided via formatted terminal tables using comfy_table. Async operations are used for external API calls (sentiment fetch), while synchronous operations handle local file/database access.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "async_function",
        "name": "main",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_init",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_config",
        "parameters": [
          {
            "description": null,
            "is_optional": true,
            "name": "key",
            "param_type": "Option<String>"
          },
          {
            "description": null,
            "is_optional": true,
            "name": "value",
            "param_type": "Option<String>"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_cash",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_cash_set",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "amount",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_cash_add",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "amount",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_portfolio",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_add",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "name",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "category",
            "param_type": "&str"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_buy",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "shares",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "price",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_sell",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "shares",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "price",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_price",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": true,
            "name": "price",
            "param_type": "Option<f64>"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "async_function",
        "name": "cmd_sentiment",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "async_function",
        "name": "cmd_report",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "cmd_history",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "limit",
            "param_type": "i64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "private"
      }
    ],
    "responsibilities": [
      "Orchestrate CLI command routing to domain-specific handlers",
      "Manage application state via configuration and database interactions",
      "Enforce financial business rules (cash balance, position validation, sentiment-based trading logic)",
      "Generate and save actionable investment reports combining sentiment, portfolio, and strategy data",
      "Provide user-facing output via formatted tables and console messages"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "config",
      "description": null,
      "file_path": "src\\config.rs",
      "functions": [],
      "importance_score": 0.9000000000000001,
      "interfaces": [
        "AppConfig",
        "Settings",
        "Allocation",
        "Thresholds",
        "BuyRatio",
        "SellRatio",
        "ApiConfig",
        "AppConfig",
        "default_config",
        "config_dir",
        "config_path",
        "db_path",
        "load",
        "validate",
        "save",
        "sentiment_zone",
        "buy_ratio_for",
        "sell_ratio_for",
        "get_value",
        "set_value"
      ],
      "name": "config.rs",
      "source_summary": "use anyhow::{Context, Result};\r\nuse serde::{Deserialize, Serialize};\r\nuse std::fs;\r\nuse std::path::PathBuf;\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct AppConfig {\r\n    pub settings: Settings,\r\n    pub allocation: Allocation,\r\n    pub thresholds: Thresholds,\r\n    pub buy_ratio: BuyRatio,\r\n    pub sell_ratio: SellRatio,\r\n    pub api: ApiConfig,\r\n}\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct Settings {\r\n    pub annualized_target_low: f64,\r\n    pub annualized_target_high: f64,\r\n    pub min_holding_days: i64,\r\n    pub min_absolute_profit_days: i64,\r\n    pub max_contrarian_weight: f64,\r\n    pub report_output_dir: String,\r\n}\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct Allocation {\r\n    pub us_stocks: f64,\r\n    pub cn_stocks: f64,\r\n    pub counter_cyclical: f64,\r\n}\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct Thresholds {\r\n    pub extreme_fear: f64,\r\n    pub fear: f64,\r\n    pub neutral: f64,\r\n    pub greed: f64,\r\n}\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct BuyRatio {\r\n    pub extreme_fear: f64,\r\n    pub fear: f64,\r\n    pub neutral: f64,\r\n    pub greed: f64,\r\n}\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct SellRatio {\r\n    pub extreme_greed_target_high: f64,\r\n    pub extreme_greed_target_low: f64,\r\n    pub extreme_greed_below_target: f64,\r\n    pub greed_target_high: f64,\r\n    pub greed_target_low: f64,\r\n    pub neutral_target_high: f64,\r\n}\r\n\r\n#[derive(Debug, Clone, Serialize, Deserialize)]\r\npub struct ApiConfig {\r\n    pub fear_greed_url: String,\r\n}\r\n\r\nimpl AppConfig {\r\n    pub fn default_config() -> Self {\r\n        Self {\r\n            settings: Settings {\r\n                annualized_target_low: 10.0,\r\n                annualized_target_high: 15.0,\r\n                min_holding_days: 30,\r\n                min_absolute_profit_days: 90,\r\n                max_contrarian_weight: 2.0,\r\n                report_output_dir: \"./reports\".to_string(),\r\n            },\r\n            allocation: Allocation {\r\n                us_stocks: 50.0,\r\n                cn_stocks: 35.0,\r\n                counter_cyclical: 15.0,\r\n            },\r\n            thresholds: Thresholds {\r\n                extreme_fear: 25.0,\r\n                fear: 45.0,\r\n                neutral: 55.0,\r\n                greed: 75.0,\r\n            },\r\n            buy_ratio: BuyRatio {\r\n                extreme_fear: 50.0,\r\n                fear: 30.0,\r\n                neutral: 20.0,\r\n                greed: 0.0,\r\n            },\r\n            sell_ratio: SellRatio {\r\n                extreme_greed_target_high: 50.0,\r\n                extreme_greed_target_low: 30.0,\r\n                extreme_greed_below_target: 20.0,\r\n                greed_target_high: 40.0,\r\n                greed_target_low: 20.0,\r\n                neutral_target_high: 30.0,\r\n            },\r\n            api: ApiConfig {\r\n                fear_greed_url: \"https://production.dataviz.cnn.io/index/fearandgreed/graphdata\"\r\n                    .to_string(),\r\n            },\r\n        }\r\n    }\r\n\r\n    pub fn config_dir() -> Result<PathBuf> {\r\n        let home = dirs::home_dir().context(\"无法获取用户主目录\")?;\r\n        Ok(home.join(\".mns\"))\r\n    }\r\n\r\n    pub fn config_path() -> Result<PathBuf> {\r\n        Ok(Self::config_dir()?.join(\"config.toml\"))\r\n    }\r\n\r\n    pub fn db_path() -> Result<PathBuf> {\r\n        Ok(Self::config_dir()?.join(\"mns.db\"))\r\n    }\r\n\r\n    pub fn load() -> Result<Self> {\r\n        let path = Self::config_path()?;\r\n        let content = fs::read_to_string(&path)\r\n            .with_context(|| format!(\"读取配置文件失败: {}\", path.display()))?;\r\n        let config: AppConfig = toml::from_str(&content).with_context(|| \"解析配置文件失败\")?;\r\n        config.validate()?;\r\n        Ok(config)\r\n    }\r\n\r\n    /// 校验配置合法性\r\n    pub fn validate(&self) -> Result<()> {\r\n        let alloc_sum = self.allocation.us_stocks + self.allocation.cn_stocks + self.allocation.counter_cyclical;\r\n        if (alloc_sum - 100.0).abs() > 0.01 {\r\n            anyhow::bail!(\r\n                \"资产配置比例之和必须为 100%，当前: 美股{}% + A股{}% + 逆周期{}% = {}%\",\r\n                self.allocation.us_stocks,\r\n                self.allocation.cn_stocks,\r\n                self.allocation.counter_cyclical,\r\n                alloc_sum\r\n            );\r\n        }\r\n        if self.settings.min_holding_days < 0 {\r\n            anyhow::bail!(\"最小持仓天数不能为负数: {}\", self.settings.min_holding_days);\r\n        }\r\n        if self.settings.max_contrarian_weight < 1.0 {\r\n            anyhow::bail!(\"最大逆向权重不能小于 1.0: {}\", self.settings.max_contrarian_weight);\r\n        }\r\n        Ok(())\r\n    }\r\n\r\n    pub fn save(&self) -> Result<()> {\r\n        let dir = Self::config_dir()?;\r\n        fs::create_dir_all(&dir)?;\r\n        let path = Self::config_path()?;\r\n        let content = toml::to_string_pretty(self)?;\r\n        fs::write(&path, content)?;\r\n        Ok(())\r\n    }\r\n\r\n    /// 根据恐贪指数判断情绪区间\r\n    pub fn sentiment_zone(&self, score: f64) -> &'static str {\r\n        if score < self.thresholds.extreme_fear {\r\n            \"Extreme Fear\"\r\n        } else if score < self.thresholds.fear {\r\n            \"Fear\"\r\n        } else if score < self.thresholds.neutral {\r\n            \"Neutral\"\r\n        } else if score < self.thresholds.greed {\r\n            \"Greed\"\r\n        } else {\r\n            \"Extreme Greed\"\r\n        }\r\n    }\r\n\r\n    /// 根据情绪区间获取买入比例\r\n    pub fn buy_ratio_for(&self, score: f64) -> f64 {\r\n        if score < self.thresholds.extreme_fear {\r\n            self.buy_ratio.extreme_fear\r\n        } else if score < self.thresholds.fear {\r\n            self.buy_ratio.fear\r\n        } else if score < self.thresholds.neutral {\r\n            self.buy_ratio.neutral\r\n        } else if score < self.thresholds.greed {\r\n            self.buy_ratio.greed\r\n        } else {\r\n            0.0 // 极度贪婪时暂停买入\r\n        }\r\n    }\r\n\r\n    /// 根据情绪区间和年化收益获取卖出减仓比例\r\n    pub fn sell_ratio_for(&self, score: f64, annualized: f64) -> f64 {\r\n        if score >= self.thresholds.greed {\r\n            // 极度贪婪\r\n            if annualized >= self.settings.annualized_target_high {\r\n                self.sell_ratio.extreme_greed_target_high\r\n            } else if annualized >= self.settings.annualized_target_low {\r\n                self.sell_ratio.extreme_greed_target_low\r\n            } else {\r\n                self.sell_ratio.extreme_greed_below_target\r\n            }\r\n        } else if score >= self.thresholds.neutral {\r\n            // 贪婪\r\n            if annualized >= self.settings.annualized_target_high {\r\n                self.sell_ratio.greed_target_high\r\n            } else if annualized >= self.settings.annualized_target_low {\r\n                self.sell_ratio.greed_target_low\r\n            } else {\r\n                0.0\r\n            }\r\n        } else if score >= self.thresholds.fear {\r\n            // 中性\r\n            if annualized >= self.settings.annualized_target_high {\r\n                self.sell_ratio.neutral_target_high\r\n            } else {\r\n                0.0\r\n            }\r\n        } else {\r\n            0.0\r\n        }\r\n    }\r\n\r\n    /// 用 dot path 获取/设置配置值\r\n    pub fn get_value(&self, key: &str) -> Option<String> {\r\n        match key {\r\n            \"settings.annualized_target_low\" => Some(self.settings.annualized_target_low.to_string()),\r\n            \"settings.annualized_target_high\" => Some(self.settings.annualized_target_high.to_string()),\r\n            \"settings.min_holding_days\" => Some(self.settings.min_holding_days.to_string()),\r\n            \"settings.min_absolute_profit_days\" => Some(self.settings.min_absolute_profit_days.to_string()),\r\n            \"settings.max_contrarian_weight\" => Some(self.settings.max_contrarian_weight.to_string()),\r\n            \"settings.report_output_dir\" => Some(self.settings.report_output_dir.clone()),\r\n            \"allocation.us_stocks\" => Some(self.allocation.us_stocks.to_string()),\r\n            \"allocation.cn_stocks\" => Some(self.allocation.cn_stocks.to_string()),\r\n            \"allocation.counter_cyclical\" => Some(self.allocation.counter_cyclical.to_string()),\r\n            \"thresholds.extreme_fear\" => Some(self.thresholds.extreme_fear.to_string()),\r\n            \"thresholds.fear\" => Some(self.thresholds.fear.to_string()),\r\n            \"thresholds.neutral\" => Some(self.thresholds.neutral.to_string()),\r\n            \"thresholds.greed\" => Some(self.thresholds.greed.to_string()),\r\n            \"buy_ratio.extreme_fear\" => Some(self.buy_ratio.extreme_fear.to_string()),\r\n            \"buy_ratio.fear\" => Some(self.buy_ratio.fear.to_string()),\r\n            \"buy_ratio.neutral\" => Some(self.buy_ratio.neutral.to_string()),\r\n            \"buy_ratio.greed\" => Some(self.buy_ratio.greed.to_string()),\r\n            \"sell_ratio.extreme_greed_target_high\" => Some(self.sell_ratio.extreme_greed_target_high.to_string()),\r\n            \"sell_ratio.extreme_greed_target_low\" => Some(self.sell_ratio.extreme_greed_target_low.to_string()),\r\n            \"sell_ratio.extreme_greed_below_target\" => Some(self.sell_ratio.extreme_greed_below_target.to_string()),\r\n            \"sell_ratio.greed_target_high\" => Some(self.sell_ratio.greed_target_high.to_string()),\r\n            \"sell_ratio.greed_target_low\" => Some(self.sell_ratio.greed_target_low.to_string()),\r\n            \"sell_ratio.neutral_target_high\" => Some(self.sell_ratio.neutral_target_high.to_string()),\r\n            \"api.fear_greed_url\" => Some(self.api.fear_greed_url.clone()),\r\n            _ => None,\r\n        }\r\n    }\r\n\r\n    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {\r\n        match key {\r\n            \"settings.annualized_target_low\" => self.settings.annualized_target_low = value.parse()?,\r\n            \"settings.annualized_target_high\" => self.settings.annualized_target_high = value.parse()?,\r\n            \"settings.min_holding_days\" => self.settings.min_holding_days = value.parse()?,\r\n            \"settings.min_absolute_profit_days\" => self.settings.min_absolute_profit_days = value.parse()?,\r\n            \"settings.max_contrarian_weight\" => self.settings.max_contrarian_weight = value.parse()?,\r\n            \"settings.report_output_dir\" => self.settings.report_output_dir = value.to_string(),\r\n            \"allocation.us_stocks\" => self.allocation.us_stocks = value.parse()?,\r\n            \"allocation.cn_stocks\" => self.allocation.cn_stocks = value.parse()?,\r\n            \"allocation.counter_cyclical\" => self.allocation.counter_cyclical = value.parse()?,\r\n            \"thresholds.extreme_fear\" => self.thresholds.extreme_fear = value.parse()?,\r\n            \"thresholds.fear\" => self.thresholds.fear = value.parse()?,\r\n            \"thresholds.neutral\" => self.thresholds.neutral = value.parse()?,\r\n            \"thresholds.greed\" => self.thresholds.greed = value.parse()?,\r\n            \"buy_ratio.extreme_fear\" => self.buy_ratio.extreme_fear = value.parse()?,\r\n            \"buy_ratio.fear\" => self.buy_ratio.fear = value.parse()?,\r\n            \"buy_ratio.neutral\" => self.buy_ratio.neutral = value.parse()?,\r\n            \"buy_ratio.greed\" => self.buy_ratio.greed = value.parse()?,\r\n            \"sell_ratio.extreme_greed_target_high\" => self.sell_ratio.extreme_greed_target_high = value.parse()?,\r\n            \"sell_ratio.extreme_greed_target_low\" => self.sell_ratio.extreme_greed_target_low = value.parse()?,\r\n            \"sell_ratio.extreme_greed_below_target\" => self.sell_ratio.extreme_greed_below_target = value.parse()?,\r\n            \"sell_ratio.greed_target_high\" => self.sell_ratio.greed_target_high = value.parse()?,\r\n            \"sell_ratio.greed_target_low\" => self.sell_ratio.greed_target_low = value.parse()?,\r\n            \"sell_ratio.neutral_target_high\" => self.sell_ratio.neutral_target_high = value.parse()?,\r\n            \"api.fear_greed_url\" => self.api.fear_greed_url = value.to_string(),\r\n            _ => anyhow::bail!(\"未知的配置项: {}\", key),\r\n        }\r\n        Ok(())\r\n    }\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 22.0,
      "lines_of_code": 282,
      "number_of_classes": 7,
      "number_of_functions": 12
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 1,
        "name": "Context",
        "path": "src\\config.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 2,
        "name": "Deserialize",
        "path": "src\\config.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 3,
        "name": "fs",
        "path": "src\\config.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 4,
        "name": "PathBuf",
        "path": "src\\config.rs",
        "version": null
      }
    ],
    "detailed_description": "The config.rs component is a centralized configuration management system for a financial trading application. It defines a hierarchical configuration structure using Rust structs (AppConfig, Settings, Allocation, Thresholds, BuyRatio, SellRatio, ApiConfig) that are serialized/deserialized via TOML using serde. The component provides comprehensive functionality for loading configuration from a file (.mns/config.toml in user's home directory), validating critical business rules (e.g., allocation sum must equal 100%), saving configuration back to disk, and exposing methods to dynamically query or modify configuration values via dot-notation paths. It also includes domain-specific logic to map fear/greed index scores to emotional zones, determine buy/sell ratios based on market sentiment and annualized returns, and manage API endpoint configuration. The component acts as the single source of truth for all application parameters, enabling both static configuration via file and dynamic runtime adjustment through programmatic interfaces.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "struct",
        "name": "AppConfig",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "Settings",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "Allocation",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "Thresholds",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "BuyRatio",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "SellRatio",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "ApiConfig",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "implementation",
        "name": "AppConfig",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "default_config",
        "parameters": [],
        "return_type": "Self",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "config_dir",
        "parameters": [],
        "return_type": "Result<PathBuf>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "config_path",
        "parameters": [],
        "return_type": "Result<PathBuf>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "db_path",
        "parameters": [],
        "return_type": "Result<PathBuf>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "load",
        "parameters": [],
        "return_type": "Result<Self>",
        "visibility": "public"
      },
      {
        "description": "校验配置合法性",
        "interface_type": "function",
        "name": "validate",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "save",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": "根据恐贪指数判断情绪区间",
        "interface_type": "function",
        "name": "sentiment_zone",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "score",
            "param_type": "f64"
          }
        ],
        "return_type": "&'static str",
        "visibility": "public"
      },
      {
        "description": "根据情绪区间获取买入比例",
        "interface_type": "function",
        "name": "buy_ratio_for",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "score",
            "param_type": "f64"
          }
        ],
        "return_type": "f64",
        "visibility": "public"
      },
      {
        "description": "根据情绪区间和年化收益获取卖出减仓比例",
        "interface_type": "function",
        "name": "sell_ratio_for",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "score",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "annualized",
            "param_type": "f64"
          }
        ],
        "return_type": "f64",
        "visibility": "public"
      },
      {
        "description": "用 dot path 获取/设置配置值",
        "interface_type": "function",
        "name": "get_value",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "key",
            "param_type": "&str"
          }
        ],
        "return_type": "Option<String>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "set_value",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "key",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "value",
            "param_type": "&str"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Manage application-wide configuration through structured, serializable data models",
      "Load and validate configuration from TOML files with strict business rule enforcement",
      "Provide dynamic runtime access and modification of configuration values via dot-path key queries",
      "Translate market sentiment scores into actionable trading signals (buy/sell ratios)",
      "Handle file system operations for configuration persistence and directory management"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "command",
      "description": null,
      "file_path": "src\\cli.rs",
      "functions": [],
      "importance_score": 0.8,
      "interfaces": [
        "Cli",
        "Commands",
        "CashAction"
      ],
      "name": "cli.rs",
      "source_summary": "use clap::{Parser, Subcommand};\r\n\r\n#[derive(Parser)]\r\n#[command(name = \"mns\", version, about = \"逆向投资助手 - Market Neutral Strategist\")]\r\npub struct Cli {\r\n    #[command(subcommand)]\r\n    pub command: Commands,\r\n}\r\n\r\n#[derive(Subcommand)]\r\npub enum Commands {\r\n    /// 初始化配置文件和数据库\r\n    Init,\r\n\r\n    /// 查看/修改配置项\r\n    Config {\r\n        /// 配置项名称 (如 thresholds.fear)\r\n        key: Option<String>,\r\n        /// 配置项新值\r\n        value: Option<String>,\r\n    },\r\n\r\n    /// 现金管理 (无子命令时查看余额)\r\n    Cash {\r\n        #[command(subcommand)]\r\n        action: Option<CashAction>,\r\n    },\r\n\r\n    /// 查看持仓概览（含年化收益）\r\n    Portfolio,\r\n\r\n    /// 新增资产到持仓池\r\n    Add {\r\n        /// 资产代码 (如 QQQ)\r\n        code: String,\r\n        /// 资产名称 (如 \"纳指100\")\r\n        name: String,\r\n        /// 类别: us_stocks / cn_stocks / counter_cyclical\r\n        category: String,\r\n    },\r\n\r\n    /// 记录买入\r\n    Buy {\r\n        /// 资产代码\r\n        code: String,\r\n        /// 买入份额\r\n        shares: f64,\r\n        /// 买入价格\r\n        price: f64,\r\n    },\r\n\r\n    /// 记录卖出\r\n    Sell {\r\n        /// 资产代码\r\n        code: String,\r\n        /// 卖出份额\r\n        shares: f64,\r\n        /// 卖出价格\r\n        price: f64,\r\n    },\r\n\r\n    /// 更新资产当前价格\r\n    Price {\r\n        /// 资产代码\r\n        code: String,\r\n        /// 当前价格 (省略则查看当前价格)\r\n        price: Option<f64>,\r\n    },\r\n\r\n    /// 查看当前恐贪指数\r\n    Sentiment,\r\n\r\n    /// 生成今日策略报告\r\n    Report,\r\n\r\n    /// 查看交易历史\r\n    History {\r\n        /// 显示条数\r\n        #[arg(default_value = \"20\")]\r\n        limit: i64,\r\n    },\r\n}\r\n\r\n#[derive(Subcommand)]\r\npub enum CashAction {\r\n    /// 设置现金余额\r\n    Set {\r\n        /// 金额\r\n        amount: f64,\r\n    },\r\n    /// 增加现金\r\n    Add {\r\n        /// 金额\r\n        amount: f64,\r\n    },\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 1.0,
      "lines_of_code": 96,
      "number_of_classes": 1,
      "number_of_functions": 0
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 1,
        "name": "Parser",
        "path": "src\\cli.rs",
        "version": null
      }
    ],
    "detailed_description": "The cli.rs component defines the command-line interface (CLI) for the 'mns' (Market Neutral Strategist) application, a tool designed to assist with reverse investing strategies. It uses the clap crate to parse user commands and arguments, organizing functionality into a hierarchical structure via subcommands. The main Cli struct contains a single Commands enum that enumerates all supported operations: Init, Config, Cash, Portfolio, Add, Buy, Sell, Price, Sentiment, Report, and History. The Cash subcommand further nests CashAction subcommands (Set, Add) to manage cash balance operations. Each command is annotated with documentation that describes its purpose and expected parameters, such as asset codes, quantities, prices, or configuration keys. The component serves as the primary entry point for user interaction, translating CLI input into actionable business logic that would be handled by downstream services (e.g., configuration manager, portfolio tracker, database layer). The low cyclomatic complexity (1.0) indicates a flat, non-branching structure, which is appropriate for a declarative CLI parser. All commands are stateless in this layer, focusing solely on argument parsing and delegation.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "struct",
        "name": "Cli",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "enum",
        "name": "Commands",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "enum",
        "name": "CashAction",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Parse and validate user commands and arguments via clap CLI framework",
      "Define hierarchical command structure for investment management operations",
      "Provide clear, documented interface for core application functionalities (portfolio, cash, sentiment, etc.)",
      "Delegate parsed commands to internal services without implementing business logic",
      "Support configuration and data management workflows through subcommand nesting (e.g., Cash -> Set/Add)"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "database",
      "description": null,
      "file_path": "src\\db.rs",
      "functions": [],
      "importance_score": 0.8,
      "interfaces": [
        "Database",
        "Database",
        "open",
        "init_tables",
        "get_cash_balance",
        "set_cash_balance",
        "add_cash",
        "add_position",
        "list_positions",
        "get_position",
        "buy_position",
        "sell_position",
        "update_price",
        "list_transactions",
        "get_latest_snapshot"
      ],
      "name": "db.rs",
      "source_summary": "use anyhow::{Context, Result};\r\nuse chrono::Local;\r\nuse rusqlite::{params, Connection};\r\n\r\nuse crate::models::{FearGreedSnapshot, Position, Transaction};\r\n\r\npub struct Database {\r\n    conn: Connection,\r\n}\r\n\r\nimpl Database {\r\n    pub fn open() -> Result<Self> {\r\n        let path = crate::config::AppConfig::db_path()?;\r\n        if let Some(parent) = path.parent() {\r\n            std::fs::create_dir_all(parent)?;\r\n        }\r\n        let conn = Connection::open(&path)\r\n            .with_context(|| format!(\"打开数据库失败: {}\", path.display()))?;\r\n        let db = Self { conn };\r\n        db.init_tables()?;\r\n        Ok(db)\r\n    }\r\n\r\n    fn init_tables(&self) -> Result<()> {\r\n        self.conn.execute_batch(\r\n            \"CREATE TABLE IF NOT EXISTS cash (\r\n                id INTEGER PRIMARY KEY CHECK (id = 1),\r\n                balance REAL NOT NULL DEFAULT 0,\r\n                updated_at TEXT NOT NULL DEFAULT (datetime('now'))\r\n            );\r\n            CREATE TABLE IF NOT EXISTS positions (\r\n                id INTEGER PRIMARY KEY AUTOINCREMENT,\r\n                asset_code TEXT NOT NULL UNIQUE,\r\n                asset_name TEXT NOT NULL,\r\n                category TEXT NOT NULL,\r\n                shares REAL NOT NULL DEFAULT 0,\r\n                cost_price REAL NOT NULL DEFAULT 0,\r\n                current_price REAL,\r\n                first_buy_date TEXT NOT NULL,\r\n                updated_at TEXT NOT NULL DEFAULT (datetime('now'))\r\n            );\r\n            CREATE TABLE IF NOT EXISTS transactions (\r\n                id INTEGER PRIMARY KEY AUTOINCREMENT,\r\n                type TEXT NOT NULL CHECK (type IN ('buy', 'sell')),\r\n                asset_code TEXT NOT NULL,\r\n                shares REAL NOT NULL,\r\n                price REAL NOT NULL,\r\n                amount REAL NOT NULL,\r\n                tx_date TEXT NOT NULL,\r\n                note TEXT\r\n            );\r\n            CREATE TABLE IF NOT EXISTS fear_greed_snapshots (\r\n                id INTEGER PRIMARY KEY AUTOINCREMENT,\r\n                score REAL NOT NULL,\r\n                rating TEXT NOT NULL,\r\n                snapshot_date TEXT NOT NULL,\r\n                previous_close REAL,\r\n                previous_1_week REAL,\r\n                previous_1_month REAL,\r\n                previous_1_year REAL,\r\n                fetched_at TEXT NOT NULL DEFAULT (datetime('now'))\r\n            );\r\n            INSERT OR IGNORE INTO cash (id, balance) VALUES (1, 0);\",\r\n        )?;\r\n        Ok(())\r\n    }\r\n\r\n    // ── Cash ──────────────────────────────────────\r\n\r\n    pub fn get_cash_balance(&self) -> Result<f64> {\r\n        let balance: f64 = self\r\n            .conn\r\n            .query_row(\"SELECT balance FROM cash WHERE id = 1\", [], |row| row.get(0))?;\r\n        Ok(balance)\r\n    }\r\n\r\n    pub fn set_cash_balance(&self, amount: f64) -> Result<()> {\r\n        if amount < 0.0 {\r\n            anyhow::bail!(\"现金余额不能为负数: {}\", amount);\r\n        }\r\n        let now = Local::now().format(\"%Y-%m-%d %H:%M:%S\").to_string();\r\n        self.conn.execute(\r\n            \"UPDATE cash SET balance = ?, updated_at = ? WHERE id = 1\",\r\n            params![amount, now],\r\n        )?;\r\n        Ok(())\r\n    }\r\n\r\n    pub fn add_cash(&self, amount: f64) -> Result<f64> {\r\n        if amount <= 0.0 {\r\n            anyhow::bail!(\"增加现金金额必须为正数: {}\", amount);\r\n        }\r\n        let balance = self.get_cash_balance()?;\r\n        let new_balance = balance + amount;\r\n        self.set_cash_balance(new_balance)?;\r\n        Ok(new_balance)\r\n    }\r\n\r\n    // ── Positions ─────────────────────────────────\r\n\r\n    pub fn add_position(&self, code: &str, name: &str, category: &str) -> Result<()> {\r\n        let today = Local::now().format(\"%Y-%m-%d\").to_string();\r\n        let now = Local::now().format(\"%Y-%m-%d %H:%M:%S\").to_string();\r\n        self.conn.execute(\r\n            \"INSERT INTO positions (asset_code, asset_name, category, shares, cost_price, first_buy_date, updated_at)\r\n             VALUES (?, ?, ?, 0, 0, ?, ?)\",\r\n            params![code, name, category, today, now],\r\n        ).with_context(|| format!(\"新增资产失败，代码 {} 可能已存在\", code))?;\r\n        Ok(())\r\n    }\r\n\r\n    pub fn list_positions(&self) -> Result<Vec<Position>> {\r\n        let mut stmt = self.conn.prepare(\r\n            \"SELECT id, asset_code, asset_name, category, shares, cost_price, current_price, first_buy_date, updated_at\r\n             FROM positions ORDER BY category, asset_code\",\r\n        )?;\r\n        let rows = stmt.query_map([], |row| {\r\n            Ok(Position {\r\n                id: row.get(0)?,\r\n                asset_code: row.get(1)?,\r\n                asset_name: row.get(2)?,\r\n                category: row.get(3)?,\r\n                shares: row.get(4)?,\r\n                cost_price: row.get(5)?,\r\n                current_price: row.get(6)?,\r\n                first_buy_date: row.get(7)?,\r\n                updated_at: row.get(8)?,\r\n            })\r\n        })?;\r\n        let mut positions = Vec::new();\r\n        for row in rows {\r\n            positions.push(row?);\r\n        }\r\n        Ok(positions)\r\n    }\r\n\r\n    pub fn get_position(&self, code: &str) -> Result<Option<Position>> {\r\n        let mut stmt = self.conn.prepare(\r\n            \"SELECT id, asset_code, asset_name, category, shares, cost_price, current_price, first_buy_date, updated_at\r\n             FROM positions WHERE asset_code = ?\",\r\n        )?;\r\n        let mut rows = stmt.query_map([code], |row| {\r\n            Ok(Position {\r\n                id: row.get(0)?,\r\n                asset_code: row.get(1)?,\r\n                asset_name: row.get(2)?,\r\n                category: row.get(3)?,\r\n                shares: row.get(4)?,\r\n                cost_price: row.get(5)?,\r\n                current_price: row.get(6)?,\r\n                first_buy_date: row.get(7)?,\r\n                updated_at: row.get(8)?,\r\n            })\r\n        })?;\r\n        match rows.next() {\r\n            Some(row) => Ok(Some(row?)),\r\n            None => Ok(None),\r\n        }\r\n    }\r\n\r\n    pub fn buy_position(&self, code: &str, shares: f64, price: f64) -> Result<()> {\r\n        if shares <= 0.0 {\r\n            anyhow::bail!(\"买入份额必须为正数\");\r\n        }\r\n        if price <= 0.0 {\r\n            anyhow::bail!(\"买入价格必须为正数\");\r\n        }\r\n\r\n        let pos = self\r\n            .get_position(code)?\r\n            .with_context(|| format!(\"未找到资产: {}\", code))?;\r\n\r\n        let now = Local::now().format(\"%Y-%m-%d %H:%M:%S\").to_string();\r\n        let today = Local::now().format(\"%Y-%m-%d\").to_string();\r\n        let amount = shares * price;\r\n\r\n        // 先检查现金余额\r\n        let balance = self.get_cash_balance()?;\r\n        if balance < amount {\r\n            anyhow::bail!(\"现金余额不足: 当前 ¥{:.2}, 需要 ¥{:.2}\", balance, amount);\r\n        }\r\n\r\n        // 更新持仓：加权平均成本\r\n        let old_total = pos.shares * pos.cost_price;\r\n        let new_shares = pos.shares + shares;\r\n        let new_cost_price = if new_shares > 0.0 {\r\n            (old_total + amount) / new_shares\r\n        } else {\r\n            price\r\n        };\r\n\r\n        let first_buy_date = if pos.shares == 0.0 {\r\n            today.clone()\r\n        } else {\r\n            pos.first_buy_date.clone()\r\n        };\r\n\r\n        // 使用事务保证原子性\r\n        let tx = self.conn.unchecked_transaction()\r\n            .with_context(|| \"开启事务失败\")?;\r\n\r\n        tx.execute(\r\n            \"UPDATE positions SET shares = ?, cost_price = ?, current_price = ?, first_buy_date = ?, updated_at = ? WHERE asset_code = ?\",\r\n            params![new_shares, new_cost_price, price, first_buy_date, now, code],\r\n        )?;\r\n\r\n        tx.execute(\r\n            \"UPDATE cash SET balance = ?, updated_at = ? WHERE id = 1\",\r\n            params![balance - amount, now],\r\n        )?;\r\n\r\n        tx.execute(\r\n            \"INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date) VALUES ('buy', ?, ?, ?, ?, ?)\",\r\n            params![code, shares, price, amount, today],\r\n        )?;\r\n\r\n        tx.commit().with_context(|| \"提交事务失败\")?;\r\n\r\n        Ok(())\r\n    }\r\n\r\n    pub fn sell_position(&self, code: &str, shares: f64, price: f64) -> Result<()> {\r\n        if shares <= 0.0 {\r\n            anyhow::bail!(\"卖出份额必须为正数\");\r\n        }\r\n        if price <= 0.0 {\r\n            anyhow::bail!(\"卖出价格必须为正数\");\r\n        }\r\n\r\n        let pos = self\r\n            .get_position(code)?\r\n            .with_context(|| format!(\"未找到资产: {}\", code))?;\r\n\r\n        if shares > pos.shares + 1e-6 {\r\n            anyhow::bail!(\"卖出份额超出持有量: 持有 {:.2}, 欲卖 {:.2}\", pos.shares, shares);\r\n        }\r\n\r\n        let actual_shares = shares.min(pos.shares); // 防止浮点误差\r\n        let now = Local::now().format(\"%Y-%m-%d %H:%M:%S\").to_string();\r\n        let today = Local::now().format(\"%Y-%m-%d\").to_string();\r\n        let amount = actual_shares * price;\r\n        let new_shares = pos.shares - actual_shares;\r\n\r\n        let balance = self.get_cash_balance()?;\r\n\r\n        // 使用事务保证原子性\r\n        let tx = self.conn.unchecked_transaction()\r\n            .with_context(|| \"开启事务失败\")?;\r\n\r\n        tx.execute(\r\n            \"UPDATE positions SET shares = ?, current_price = ?, updated_at = ? WHERE asset_code = ?\",\r\n            params![new_shares, price, now, code],\r\n        )?;\r\n\r\n        tx.execute(\r\n            \"UPDATE cash SET balance = ?, updated_at = ? WHERE id = 1\",\r\n            params![balance + amount, now],\r\n        )?;\r\n\r\n        tx.execute(\r\n            \"INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date) VALUES ('sell', ?, ?, ?, ?, ?)\",\r\n            params![code, actual_shares, price, amount, today],\r\n        )?;\r\n\r\n        tx.commit().with_context(|| \"提交事务失败\")?;\r\n\r\n        Ok(())\r\n    }\r\n\r\n    pub fn update_price(&self, code: &str, price: f64) -> Result<()> {\r\n        let now = Local::now().format(\"%Y-%m-%d %H:%M:%S\").to_string();\r\n        let rows = self.conn.execute(\r\n            \"UPDATE positions SET current_price = ?, updated_at = ? WHERE asset_code = ?\",\r\n            params![price, now, code],\r\n        )?;\r\n        if rows == 0 {\r\n            anyhow::bail!(\"未找到资产: {}\", code);\r\n        }\r\n        Ok(())\r\n    }\r\n\r\n    // ── Transactions ──────────────────────────────\r\n\r\n    pub fn list_transactions(&self, limit: i64) -> Result<Vec<Transaction>> {\r\n        let mut stmt = self.conn.prepare(\r\n            \"SELECT id, type, asset_code, shares, price, amount, tx_date, note\r\n             FROM transactions ORDER BY id DESC LIMIT ?\",\r\n        )?;\r\n        let rows = stmt.query_map([limit], |row| {\r\n            Ok(Transaction {\r\n                id: row.get(0)?,\r\n                tx_type: row.get(1)?,\r\n                asset_code: row.get(2)?,\r\n                shares: row.get(3)?,\r\n                price: row.get(4)?,\r\n                amount: row.get(5)?,\r\n                tx_date: row.get(6)?,\r\n                note: row.get(7)?,\r\n            })\r\n        })?;\r\n        let mut txs = Vec::new();\r\n        for row in rows {\r\n            txs.push(row?);\r\n        }\r\n        Ok(txs)\r\n    }\r\n\r\n    // ── Fear & Greed Snapshots ────────────────────\r\n\r\n    pub fn save_fear_greed_snapshot(\r\n        &self,\r\n        score: f64,\r\n        rating: &str,\r\n        previous_close: Option<f64>,\r\n        previous_1_week: Option<f64>,\r\n        previous_1_month: Option<f64>,\r\n        previous_1_year: Option<f64>,\r\n    ) -> Result<()> {\r\n        let today = Local::now().format(\"%Y-%m-%d\").to_string();\r\n        let now = Local::now().format(\"%Y-%m-%d %H:%M:%S\").to_string();\r\n        // 同一天只保留最新快照：先删除当天已有记录，再插入\r\n        self.conn.execute(\r\n            \"DELETE FROM fear_greed_snapshots WHERE snapshot_date = ?\",\r\n            params![today],\r\n        )?;\r\n        self.conn.execute(\r\n            \"INSERT INTO fear_greed_snapshots (score, rating, snapshot_date, previous_close, previous_1_week, previous_1_month, previous_1_year, fetched_at)\r\n             VALUES (?, ?, ?, ?, ?, ?, ?, ?)\",\r\n            params![score, rating, today, previous_close, previous_1_week, previous_1_month, previous_1_year, now],\r\n        )?;\r\n        Ok(())\r\n    }\r\n\r\n    #[allow(dead_code)]\r\n    pub fn get_latest_snapshot(&self) -> Result<Option<FearGreedSnapshot>> {\r\n        let mut stmt = self.conn.prepare(\r\n            \"SELECT id, score, rating, snapshot_date, previous_close, previous_1_week, previous_1_month, previous_1_year, fetched_at\r\n             FROM fear_greed_snapshots ORDER BY id DESC LIMIT 1\",\r\n        )?;\r\n        let mut rows = stmt.query_map([], |row| {\r\n            Ok(FearGreedSnapshot {\r\n                id: row.get(0)?,\r\n                score: row.get(1)?,\r\n                rating: row.get(2)?,\r\n                snapshot_date: row.get(3)?,\r\n                previous_close: row.get(4)?,\r\n                previous_1_week: row.get(5)?,\r\n                previous_1_month: row.get(6)?,\r\n                previous_1_year: row.get(7)?,\r\n                fetched_at: row.get(8)?,\r\n            })\r\n        })?;\r\n        match rows.next() {\r\n            Some(row) => Ok(Some(row?)),\r\n            None => Ok(None),\r\n        }\r\n    }\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 17.0,
      "lines_of_code": 358,
      "number_of_classes": 1,
      "number_of_functions": 14
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 1,
        "name": "Context",
        "path": "src\\db.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 2,
        "name": "Local",
        "path": "src\\db.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 3,
        "name": "params",
        "path": "src\\db.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 5,
        "name": "FearGreedSnapshot",
        "path": "src\\db.rs",
        "version": null
      }
    ],
    "detailed_description": "The db.rs component is a SQLite-based database manager that handles persistent storage for a financial portfolio tracking application. It manages four main data entities: cash balance, investment positions, transaction history, and fear & greed market sentiment snapshots. The component initializes the database schema on first use, ensuring tables exist with appropriate constraints and default values. It provides comprehensive CRUD operations for each entity, including atomic transactions for buy/sell operations that update both position holdings and cash balance simultaneously. The component uses rusqlite for database interactions, chrono for timestamp generation, and anyhow for error handling. Key business logic includes weighted average cost calculation for stock purchases, float precision handling during sell operations, and daily deduplication of market sentiment data. All database operations are wrapped in robust error handling with contextual messages, and transactions are explicitly managed to ensure data consistency across related tables.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "struct",
        "name": "Database",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "implementation",
        "name": "Database",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "open",
        "parameters": [],
        "return_type": "Result<Self>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "init_tables",
        "parameters": [],
        "return_type": "Result<()>",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "get_cash_balance",
        "parameters": [],
        "return_type": "Result<f64>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "set_cash_balance",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "amount",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "add_cash",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "amount",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<f64>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "add_position",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "name",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "category",
            "param_type": "&str"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "list_positions",
        "parameters": [],
        "return_type": "Result<Vec<Position>>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "get_position",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          }
        ],
        "return_type": "Result<Option<Position>>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "buy_position",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "shares",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "price",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "sell_position",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "shares",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "price",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "update_price",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "code",
            "param_type": "&str"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "price",
            "param_type": "f64"
          }
        ],
        "return_type": "Result<()>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "list_transactions",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "limit",
            "param_type": "i64"
          }
        ],
        "return_type": "Result<Vec<Transaction>>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "get_latest_snapshot",
        "parameters": [],
        "return_type": "Result<Option<FearGreedSnapshot>>",
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Manage persistent storage of cash balance and enforce non-negative balance constraints",
      "Track and update investment positions with weighted average cost calculation and date tracking",
      "Record and retrieve buy/sell transactions with atomic updates to both positions and cash",
      "Store and deduplicate daily fear & greed market sentiment snapshots with historical context",
      "Initialize and maintain database schema with proper constraints and defaults"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "model",
      "description": null,
      "file_path": "src\\models.rs",
      "functions": [],
      "importance_score": 0.8,
      "interfaces": [
        "Position",
        "Position",
        "market_value",
        "market_value_or_cost",
        "annualized_return",
        "annualized_return_with_min_days",
        "absolute_return",
        "Transaction",
        "FearGreedSnapshot",
        "FearGreedResponse",
        "FearGreedData"
      ],
      "name": "models.rs",
      "source_summary": "use chrono::NaiveDate;\r\n\r\n#[derive(Debug, Clone)]\r\n#[allow(dead_code)]\r\npub struct Position {\r\n    pub id: i64,\r\n    pub asset_code: String,\r\n    pub asset_name: String,\r\n    pub category: String,\r\n    pub shares: f64,\r\n    pub cost_price: f64,\r\n    pub current_price: Option<f64>,\r\n    pub first_buy_date: String,\r\n    pub updated_at: String,\r\n}\r\n\r\nimpl Position {\r\n    /// 持仓市值，无现价时返回 None\r\n    #[allow(dead_code)]\r\n    pub fn market_value(&self) -> Option<f64> {\r\n        self.current_price.map(|p| p * self.shares)\r\n    }\r\n\r\n    /// 持仓市值，无现价时使用成本价估算（用于总资产等不可缺失场景）\r\n    pub fn market_value_or_cost(&self) -> f64 {\r\n        self.current_price\r\n            .map(|p| p * self.shares)\r\n            .unwrap_or(self.cost_price * self.shares)\r\n    }\r\n\r\n    /// 年化收益（无最小天数限制），保留作为通用 API\r\n    #[allow(dead_code)]\r\n    pub fn annualized_return(&self, today: &NaiveDate) -> Option<f64> {\r\n        self.annualized_return_with_min_days(today, 0)\r\n    }\r\n\r\n    /// 年化收益计算，可指定最小持仓天数门槛\r\n    /// 持仓天数不足门槛时不返回年化值，避免短期收益被放大失真\r\n    /// 正收益：使用复利公式 (current/cost)^(1/years) - 1\r\n    /// 负收益：使用简单年化 (current/cost - 1) / years，避免复利公式对亏损的过度放大\r\n    pub fn annualized_return_with_min_days(&self, today: &NaiveDate, min_days: i64) -> Option<f64> {\r\n        let cost = self.cost_price;\r\n        let current = self.current_price?;\r\n        if cost <= 0.0 || current <= 0.0 {\r\n            return None;\r\n        }\r\n        let first = NaiveDate::parse_from_str(&self.first_buy_date, \"%Y-%m-%d\").ok()?;\r\n        let days = (*today - first).num_days();\r\n        if days <= 0 {\r\n            return None;\r\n        }\r\n        if days < min_days {\r\n            return None;\r\n        }\r\n        let years = days as f64 / 365.0;\r\n        let ratio = current / cost;\r\n        if ratio >= 1.0 {\r\n            // 正收益：复利公式\r\n            Some(ratio.powf(1.0 / years) - 1.0)\r\n        } else {\r\n            // 负收益：简单年化，避免 (0.95)^12 - 1 ≈ -46% 的失真\r\n            Some((ratio - 1.0) / years)\r\n        }\r\n    }\r\n\r\n    /// 绝对收益率 (不考虑时间)\r\n    pub fn absolute_return(&self) -> Option<f64> {\r\n        let cost = self.cost_price;\r\n        let current = self.current_price?;\r\n        if cost <= 0.0 {\r\n            return None;\r\n        }\r\n        Some((current - cost) / cost)\r\n    }\r\n}\r\n\r\n#[derive(Debug, Clone)]\r\n#[allow(dead_code)]\r\npub struct Transaction {\r\n    pub id: i64,\r\n    pub tx_type: String,\r\n    pub asset_code: String,\r\n    pub shares: f64,\r\n    pub price: f64,\r\n    pub amount: f64,\r\n    pub tx_date: String,\r\n    pub note: Option<String>,\r\n}\r\n\r\n#[derive(Debug, Clone)]\r\n#[allow(dead_code)]\r\npub struct FearGreedSnapshot {\r\n    pub id: i64,\r\n    pub score: f64,\r\n    pub rating: String,\r\n    pub snapshot_date: String,\r\n    pub previous_close: Option<f64>,\r\n    pub previous_1_week: Option<f64>,\r\n    pub previous_1_month: Option<f64>,\r\n    pub previous_1_year: Option<f64>,\r\n    pub fetched_at: String,\r\n}\r\n\r\n#[derive(Debug, Clone, serde::Deserialize)]\r\npub struct FearGreedResponse {\r\n    pub fear_and_greed: FearGreedData,\r\n}\r\n\r\n#[derive(Debug, Clone, serde::Deserialize)]\r\n#[allow(dead_code)]\r\npub struct FearGreedData {\r\n    pub score: f64,\r\n    pub rating: String,\r\n    pub timestamp: String,\r\n    #[serde(default)]\r\n    pub previous_close: Option<f64>,\r\n    #[serde(default)]\r\n    pub previous_1_week: Option<f64>,\r\n    #[serde(default)]\r\n    pub previous_1_month: Option<f64>,\r\n    #[serde(default)]\r\n    pub previous_1_year: Option<f64>,\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 6.0,
      "lines_of_code": 123,
      "number_of_classes": 5,
      "number_of_functions": 5
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 1,
        "name": "NaiveDate",
        "path": "src\\models.rs",
        "version": null
      }
    ],
    "detailed_description": "The component in models.rs defines five data models that represent core financial entities in a portfolio management system: Position, Transaction, FearGreedSnapshot, FearGreedResponse, and FearGreedData. The Position model encapsulates holding information including asset details, cost, current price, and purchase date, with rich business logic for calculating market value, absolute return, and annualized return (with a sophisticated dual-mode calculation that applies compound growth for positive returns and simple linear annualization for losses to avoid distortion). The Transaction model captures trade records for audit and reconciliation. The FearGreed* models handle external market sentiment data from a third-party API, with serde annotations enabling automatic deserialization from JSON. The component is designed for data persistence and financial computation, serving as the foundational data layer for portfolio analytics. All models are marked as Debug and Clone for ease of testing and internal use, with dead_code allowances likely for unused fields during development or partial serialization scenarios.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "struct",
        "name": "Position",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "implementation",
        "name": "Position",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "market_value",
        "parameters": [],
        "return_type": "Option<f64>",
        "visibility": "public"
      },
      {
        "description": "持仓市值，无现价时使用成本价估算（用于总资产等不可缺失场景）",
        "interface_type": "function",
        "name": "market_value_or_cost",
        "parameters": [],
        "return_type": "f64",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "function",
        "name": "annualized_return",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "today",
            "param_type": "&NaiveDate"
          }
        ],
        "return_type": "Option<f64>",
        "visibility": "public"
      },
      {
        "description": "年化收益计算，可指定最小持仓天数门槛 持仓天数不足门槛时不返回年化值，避免短期收益被放大失真 正收益：使用复利公式 (current/cost)^(1/years) - 1 负收益：使用简单年化 (current/cost - 1) / years，避免复利公式对亏损的过度放大",
        "interface_type": "function",
        "name": "annualized_return_with_min_days",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "today",
            "param_type": "&NaiveDate"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "min_days",
            "param_type": "i64"
          }
        ],
        "return_type": "Option<f64>",
        "visibility": "public"
      },
      {
        "description": "绝对收益率 (不考虑时间)",
        "interface_type": "function",
        "name": "absolute_return",
        "parameters": [],
        "return_type": "Option<f64>",
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "Transaction",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "FearGreedSnapshot",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "FearGreedResponse",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "FearGreedData",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Define and structure financial data models for portfolio holdings (Position) and transactions (Transaction)",
      "Implement business logic for financial metrics including market value, absolute return, and context-aware annualized return calculations",
      "Model external market sentiment data (FearGreed) with proper deserialization via serde for API integration",
      "Provide type-safe, immutable data containers that support computation without side effects",
      "Ensure consistency in date and numeric field handling across models to support reliable analytics"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "service",
      "description": null,
      "file_path": "src\\report.rs",
      "functions": [],
      "importance_score": 0.8,
      "interfaces": [
        "save_report"
      ],
      "name": "report.rs",
      "source_summary": "use anyhow::Result;\r\nuse chrono::{Datelike, Local};\r\nuse std::fs;\r\nuse std::path::Path;\r\n\r\nuse crate::config::AppConfig;\r\nuse crate::models::Position;\r\nuse crate::strategy::{BuySuggestion, RiskAdvice, RiskWarning, SellReason, SellSuggestion};\r\n\r\npub fn generate_report(\r\n    config: &AppConfig,\r\n    score: f64,\r\n    rating: &str,\r\n    previous_close: Option<f64>,\r\n    previous_1_week: Option<f64>,\r\n    previous_1_month: Option<f64>,\r\n    previous_1_year: Option<f64>,\r\n    cash_balance: f64,\r\n    positions: &[Position],\r\n    buy_suggestion: &BuySuggestion,\r\n    sell_suggestions: &[SellSuggestion],\r\n    risk_warnings: &[RiskWarning],\r\n) -> Result<String> {\r\n    let today = Local::now();\r\n    let weekday = match today.weekday().num_days_from_monday() {\r\n        0 => \"Monday\",\r\n        1 => \"Tuesday\",\r\n        2 => \"Wednesday\",\r\n        3 => \"Thursday\",\r\n        4 => \"Friday\",\r\n        5 => \"Saturday\",\r\n        _ => \"Sunday\",\r\n    };\r\n    let date_str = today.format(\"%Y-%m-%d\").to_string();\r\n\r\n    let mut report = String::new();\r\n\r\n    // Header\r\n    report.push_str(&format!(\r\n        \"═══════════════════════════════════════════════════\\n\\\r\n         逆向投资助手 - 每日策略报告\\n\\\r\n         {} ({})\\n\\\r\n         ═════════════════════════════════════════════════\\n\\n\",\r\n        date_str, weekday\r\n    ));\r\n\r\n    // 市场情绪\r\n    report.push_str(\"【市场情绪】\\n\");\r\n    report.push_str(&format!(\"  CNN 恐贪指数: {:.2} ({})\\n\", score, rating));\r\n    if let Some(pc) = previous_close {\r\n        report.push_str(&format!(\"  前日收盘: {:.2}\", pc));\r\n    }\r\n    if let Some(pw) = previous_1_week {\r\n        report.push_str(&format!(\" | 周环比: {:.2} → {:.2}\", pw, score));\r\n    }\r\n    report.push('\\n');\r\n    if let Some(pm) = previous_1_month {\r\n        report.push_str(&format!(\"  月环比: {:.2} → {:.2}\", pm, score));\r\n    }\r\n    if let Some(py) = previous_1_year {\r\n        report.push_str(&format!(\" | 年同比: {:.2} → {:.2}\", py, score));\r\n    }\r\n    report.push_str(\"\\n\\n\");\r\n\r\n    // 账户概览\r\n    let total_mv: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum();\r\n    let total_assets = cash_balance + total_mv;\r\n    let today_date = today.date_naive();\r\n\r\n    report.push_str(\"【账户概览】\\n\");\r\n    report.push_str(&format!(\"  现金余额: ¥{:.2}\\n\", cash_balance));\r\n    report.push_str(&format!(\"  持仓市值: ¥{:.2}\\n\", total_mv));\r\n    report.push_str(&format!(\"  总资产:   ¥{:.2}\\n\\n\", total_assets));\r\n\r\n    // 持仓明细\r\n    if !positions.is_empty() {\r\n        report.push_str(\"  持仓明细:\\n\");\r\n        report.push_str(\"  ┌──────────┬──────────────┬──────────┬──────────┬──────────┬──────────┬──────────┐\\n\");\r\n        report.push_str(\"  │ 代码     │ 名称         │ 份额     │ 成本价   │ 现价     │ 年化收益 │ 绝对收益 │\\n\");\r\n        report.push_str(\"  ├──────────┼──────────────┼──────────┼──────────┼──────────┼──────────┤──────────┤\\n\");\r\n\r\n        for pos in positions {\r\n            let ann_str = match pos.annualized_return_with_min_days(&today_date, config.settings.min_holding_days) {\r\n                Some(r) => format!(\"{:+.1}%\", r * 100.0),\r\n                None => \"N/A\".to_string(),\r\n            };\r\n            let abs_str = match pos.absolute_return() {\r\n                Some(r) => format!(\"{:+.1}%\", r * 100.0),\r\n                None => \"N/A\".to_string(),\r\n            };\r\n            let cur_str = match pos.current_price {\r\n                Some(p) => format!(\"{:.2}\", p),\r\n                None => \"-\".to_string(),\r\n            };\r\n            report.push_str(&format!(\r\n                \"  │ {:<8} │ {:<12} │ {:>8.2} │ {:>8.2} │ {:>8} │ {:>8} │ {:>8} │\\n\",\r\n                pos.asset_code, pos.asset_name, pos.shares, pos.cost_price, cur_str, ann_str, abs_str\r\n            ));\r\n        }\r\n        report.push_str(\"  └──────────┴──────────────┴──────────┴──────────┴──────────┴──────────┴──────────┘\\n\\n\");\r\n    }\r\n\r\n    // 卖出建议\r\n    if !sell_suggestions.is_empty() {\r\n        report.push_str(\"【卖出建议】\\n\");\r\n        for s in sell_suggestions {\r\n            let reason_str = match &s.reason {\r\n                SellReason::AnnualizedHigh => {\r\n                    if let Some(ann) = s.annualized_return {\r\n                        if ann * 100.0 >= config.settings.annualized_target_high {\r\n                            format!(\"年化 {:.1}% ≥ {}% 高线\", ann * 100.0, config.settings.annualized_target_high)\r\n                        } else if ann * 100.0 >= config.settings.annualized_target_low {\r\n                            format!(\"年化 {:.1}% ≥ {}% 低线\", ann * 100.0, config.settings.annualized_target_low)\r\n                        } else {\r\n                            \"情绪驱动减仓\".to_string()\r\n                        }\r\n                    } else {\r\n                        \"情绪驱动减仓\".to_string()\r\n                    }\r\n                }\r\n                SellReason::AbsoluteProfit => {\r\n                    format!(\"绝对收益 {:.0}%（长期持有获利了结）\", s.absolute_return * 100.0)\r\n                }\r\n            };\r\n            report.push_str(&format!(\r\n                \"  ▸ {} ({}) — {}\\n\",\r\n                s.asset_code, s.asset_name, reason_str\r\n            ));\r\n            report.push_str(&format!(\r\n                \"    建议: 减仓 {:.0}%，卖出 {:.2} 份，预计回收 ¥{:.2}\\n\",\r\n                s.sell_ratio, s.sell_shares, s.sell_amount\r\n            ));\r\n        }\r\n        report.push('\\n');\r\n    }\r\n\r\n    // 买入建议\r\n    report.push_str(\"【买入建议】\\n\");\r\n    if buy_suggestion.total_amount > 0.0 {\r\n        let zone = config.sentiment_zone(score);\r\n        let sell_proceeds: f64 = sell_suggestions.iter().map(|s| s.sell_amount).sum();\r\n        let effective_cash = cash_balance + sell_proceeds;\r\n        report.push_str(&format!(\r\n            \"  当前市场\\\"{}\\\"，建议投入 ¥{:.2}（可用资金 ¥{:.2} 的 {:.0}%）\\n\",\r\n            zone, buy_suggestion.total_amount, effective_cash, config.buy_ratio_for(score)\r\n        ));\r\n        if sell_proceeds > 0.0 {\r\n            report.push_str(&format!(\r\n                \"  注: 可用资金含卖出回收 ¥{:.2}\\n\",\r\n                sell_proceeds\r\n            ));\r\n        }\r\n        report.push_str(&format!(\r\n            \"    - 美股 ¥{:.2} | A股 ¥{:.2} | 逆周期 ¥{:.2}\\n\",\r\n            buy_suggestion.us_amount, buy_suggestion.cn_amount, buy_suggestion.counter_amount\r\n        ));\r\n        if !buy_suggestion.details.is_empty() {\r\n            report.push_str(\"  分配明细（逆向加权：浮亏标的获得更多资金）:\\n\");\r\n            for d in &buy_suggestion.details {\r\n                report.push_str(&format!(\"    · {} ({}): ¥{:.2}\\n\", d.asset_code, d.asset_name, d.amount));\r\n            }\r\n        }\r\n        if !buy_suggestion.excluded.is_empty() {\r\n            report.push_str(\"  以下标的因高浮亏暂停加仓:\\n\");\r\n            for e in &buy_suggestion.excluded {\r\n                report.push_str(&format!(\"    ✗ {} ({}) — 浮亏 {:.0}%: {}\\n\", e.asset_code, e.asset_name, e.loss_ratio, e.reason));\r\n            }\r\n        }\r\n    } else {\r\n        report.push_str(\"  当前市场情绪偏高，建议暂停买入。\\n\");\r\n        report.push_str(\"  可用资金继续持有，等待市场回调。\\n\");\r\n    }\r\n    report.push('\\n');\r\n\r\n    // 净操作指引\r\n    let total_sell: f64 = sell_suggestions.iter().map(|s| s.sell_amount).sum();\r\n    let net_flow = buy_suggestion.total_amount - total_sell;\r\n    report.push_str(\"【净操作指引】\\n\");\r\n    if net_flow > 0.0 {\r\n        report.push_str(&format!(\r\n            \"  今日净买入 ¥{:.2}（买入 ¥{:.2} - 卖出 ¥{:.2}）\\n\",\r\n            net_flow, buy_suggestion.total_amount, total_sell\r\n        ));\r\n        report.push_str(\"  操作方向: 加仓，整体偏逆向买入\\n\");\r\n    } else if net_flow < 0.0 {\r\n        report.push_str(&format!(\r\n            \"  今日净卖出 ¥{:.2}（买入 ¥{:.2} - 卖出 ¥{:.2}）\\n\",\r\n            -net_flow, buy_suggestion.total_amount, total_sell\r\n        ));\r\n        report.push_str(\"  操作方向: 减仓，获利了结为主\\n\");\r\n    } else if buy_suggestion.total_amount > 0.0 {\r\n        report.push_str(\"  买入与卖出金额基本持平，维持当前仓位\\n\");\r\n    } else {\r\n        report.push_str(\"  今日无操作建议，持有观望\\n\");\r\n    }\r\n    report.push('\\n');\r\n\r\n    // 风险警告\r\n    if !risk_warnings.is_empty() {\r\n        report.push_str(\"【风险警告】\\n\");\r\n        for w in risk_warnings {\r\n            let advice_str = match &w.advice {\r\n                RiskAdvice::ConsiderBuyMore => \"恐慌环境下浮亏，可能是加仓机会——若基本面未恶化，可考虑逆向加仓\",\r\n                RiskAdvice::ReviewFundamentals => \"中性环境下浮亏，建议审视基本面是否恶化\",\r\n                RiskAdvice::UrgentReview => \"贪婪环境下仍浮亏，需紧急审视——市场普涨时该标的逆势下跌，可能存在结构性问题\",\r\n            };\r\n            report.push_str(&format!(\r\n                \"  ▸ {} ({}) — 浮亏 {:.1}%\\n\",\r\n                w.asset_code, w.asset_name, w.loss_ratio\r\n            ));\r\n            report.push_str(&format!(\"    {}\\n\", advice_str));\r\n        }\r\n        report.push('\\n');\r\n    }\r\n\r\n    // 资金分配预案\r\n    let sell_proceeds_for_plan: f64 = sell_suggestions.iter().map(|s| s.sell_amount).sum();\r\n    let effective_cash_for_plan = cash_balance + sell_proceeds_for_plan;\r\n\r\n    report.push_str(\"【资金分配预案】\\n\");\r\n    report.push_str(\"  若市场回调至不同区间的投入预案:\\n\");\r\n    if sell_proceeds_for_plan > 0.0 {\r\n        report.push_str(&format!(\r\n            \"  注: 预案基于可用资金 ¥{:.2}（现金 ¥{:.2} + 卖出回收 ¥{:.2}）\\n\",\r\n            effective_cash_for_plan, cash_balance, sell_proceeds_for_plan\r\n        ));\r\n    }\r\n\r\n    let zones = [\r\n        (\"极度恐慌\", config.thresholds.extreme_fear, config.buy_ratio.extreme_fear),\r\n        (\"恐慌\", config.thresholds.fear, config.buy_ratio.fear),\r\n        (\"中性\", config.thresholds.neutral, config.buy_ratio.neutral),\r\n        (\"贪婪\", config.thresholds.greed, config.buy_ratio.greed),\r\n    ];\r\n\r\n    for (i, (name, threshold, ratio)) in zones.iter().enumerate() {\r\n        let amount = effective_cash_for_plan * (ratio / 100.0);\r\n        // 极度恐慌: 指数 < threshold; 其他: 指数 < threshold 的该区间\r\n        let threshold_desc = if i == 0 {\r\n            format!(\"指数 < {:.0}\", threshold)\r\n        } else {\r\n            format!(\"{:.0} ≤ 指数 < {:.0}\", zones[i - 1].1, threshold)\r\n        };\r\n        if *ratio > 0.0 {\r\n            report.push_str(&format!(\r\n                \"  · {}   ({}): 投入 ¥{:.2} ({:.0}%)\\n\",\r\n                name, threshold_desc, amount, ratio\r\n            ));\r\n            report.push_str(&format!(\r\n                \"    - 美股 ¥{:.2} | A股 ¥{:.2} | 逆周期 ¥{:.2}\\n\",\r\n                amount * config.allocation.us_stocks / 100.0,\r\n                amount * config.allocation.cn_stocks / 100.0,\r\n                amount * config.allocation.counter_cyclical / 100.0,\r\n            ));\r\n        } else {\r\n            report.push_str(&format!(\r\n                \"  · {}   ({}): 暂停买入，持有观望\\n\",\r\n                name, threshold_desc\r\n            ));\r\n        }\r\n    }\r\n\r\n    report.push_str(\"\\n═══════════════════════════════════════════════════\\n\");\r\n\r\n    Ok(report)\r\n}\r\n\r\npub fn save_report(config: &AppConfig, content: &str) -> Result<String> {\r\n    let today = Local::now().format(\"%Y-%m-%d\").to_string();\r\n    let output_dir = &config.settings.report_output_dir;\r\n    fs::create_dir_all(output_dir)?;\r\n\r\n    let filepath = Path::new(output_dir).join(format!(\"{}.txt\", today));\r\n    fs::write(&filepath, content)?;\r\n\r\n    Ok(filepath.to_string_lossy().to_string())\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 33.0,
      "lines_of_code": 277,
      "number_of_classes": 0,
      "number_of_functions": 2
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 1,
        "name": "Result",
        "path": "src\\report.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 2,
        "name": "Datelike",
        "path": "src\\report.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 3,
        "name": "fs",
        "path": "src\\report.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 4,
        "name": "Path",
        "path": "src\\report.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 6,
        "name": "AppConfig",
        "path": "src\\report.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 7,
        "name": "Position",
        "path": "src\\report.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 8,
        "name": "BuySuggestion",
        "path": "src\\report.rs",
        "version": null
      }
    ],
    "detailed_description": "The report.rs component is a service component responsible for generating and saving a comprehensive daily investment strategy report. It takes in market sentiment scores, account data (cash balance, positions), buy/sell suggestions, and risk warnings to produce a human-readable, formatted report in Chinese. The report includes sections on market sentiment (CNN Fear & Greed Index), account overview, detailed position holdings, buy/sell recommendations with rationale, net operation guidance, risk warnings for underperforming assets, and a dynamic capital allocation预案 based on market zones. The component uses the chrono crate for date formatting, std::fs for file operations, and integrates with application configuration to dynamically adjust thresholds and ratios. It formats output with ASCII tables for readability and supports conditional logic based on configuration settings like minimum holding days, annualized return targets, and asset allocation ratios. The save_report function persists the generated report to a timestamped file in a configured directory, enabling audit trails and historical analysis.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "function",
        "name": "save_report",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "config",
            "param_type": "&AppConfig"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "content",
            "param_type": "&str"
          }
        ],
        "return_type": "Result<String>",
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Generate a structured, human-readable daily investment report with market sentiment, account status, and actionable recommendations",
      "Calculate and display position performance metrics including annualized and absolute returns based on configuration thresholds",
      "Formulate buy and sell suggestions using configurable rules tied to market sentiment, return targets, and capital availability",
      "Provide risk warnings for underperforming assets with tailored advice based on current market conditions",
      "Save the generated report to disk in a timestamped file for record-keeping and user access"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "service",
      "description": null,
      "file_path": "src\\sentiment.rs",
      "functions": [],
      "importance_score": 0.8,
      "interfaces": [
        "fetch_fear_greed"
      ],
      "name": "sentiment.rs",
      "source_summary": "use anyhow::{Context, Result};\r\nuse crate::config::AppConfig;\r\nuse crate::models::FearGreedResponse;\r\n\r\npub async fn fetch_fear_greed(config: &AppConfig) -> Result<FearGreedResponse> {\r\n    let client = reqwest::Client::builder()\r\n        .user_agent(\"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36\")\r\n        .connect_timeout(std::time::Duration::from_secs(10))\r\n        .timeout(std::time::Duration::from_secs(30))\r\n        .build()?;\r\n\r\n    let resp = client\r\n        .get(&config.api.fear_greed_url)\r\n        .header(\"Accept\", \"application/json, text/plain, */*\")\r\n        .header(\"Accept-Language\", \"en-US,en;q=0.9\")\r\n        .header(\"Referer\", \"https://www.cnn.com/markets/fear-and-greed\")\r\n        .send()\r\n        .await\r\n        .context(\"请求 CNN 恐贪指数 API 失败\")?;\r\n\r\n    if !resp.status().is_success() {\r\n        let status = resp.status();\r\n        anyhow::bail!(\"API 请求失败，状态码: {}\", status);\r\n    }\r\n\r\n    let data: FearGreedResponse = resp\r\n        .json()\r\n        .await\r\n        .context(\"解析 CNN 恐贪指数响应失败\")?;\r\n\r\n    Ok(data)\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 2.0,
      "lines_of_code": 32,
      "number_of_classes": 0,
      "number_of_functions": 2
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 1,
        "name": "Context",
        "path": "src\\sentiment.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 2,
        "name": "AppConfig",
        "path": "src\\sentiment.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 3,
        "name": "FearGreedResponse",
        "path": "src\\sentiment.rs",
        "version": null
      }
    ],
    "detailed_description": "The component is a service function named `fetch_fear_greed` that retrieves the CNN Fear & Greed Index data from a remote API. It constructs an HTTP client with specific headers (User-Agent, Accept, Accept-Language, Referer) and timeouts to ensure compatibility and reliability when querying the CNN API endpoint defined in the application configuration. The function sends a GET request to the configured URL, validates the HTTP response status, and deserializes the JSON response into a `FearGreedResponse` model. Error handling is implemented using the `anyhow` crate to provide contextualized error messages for network failures, non-2xx responses, and JSON parsing errors. This component abstracts the external API interaction, enabling other parts of the system to consume sentiment data without managing HTTP details.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "async_function",
        "name": "fetch_fear_greed",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "config",
            "param_type": "&AppConfig"
          }
        ],
        "return_type": "Result<FearGreedResponse>",
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Construct and configure an HTTP client with appropriate timeouts and headers for API communication",
      "Send a GET request to the CNN Fear & Greed Index API endpoint using configuration-provided URL",
      "Validate HTTP response status and fail with descriptive error if non-successful",
      "Deserialize JSON response into the `FearGreedResponse` model with proper error context",
      "Abstract external API interaction to provide a clean, async interface for higher-level components"
    ]
  },
  {
    "code_dossier": {
      "code_purpose": "service",
      "description": null,
      "file_path": "src\\strategy.rs",
      "functions": [],
      "importance_score": 0.8,
      "interfaces": [
        "BuySuggestion",
        "BuyDetail",
        "SellSuggestion",
        "SellReason",
        "RiskWarning",
        "RiskAdvice",
        "total_sell_proceeds",
        "ExcludedFromBuy",
        "distribute_amount_contrarian",
        "check_risk_warnings"
      ],
      "name": "strategy.rs",
      "source_summary": "use crate::config::AppConfig;\r\nuse crate::models::Position;\r\nuse chrono::{Local, NaiveDate};\r\n\r\n#[derive(Debug)]\r\npub struct BuySuggestion {\r\n    pub total_amount: f64,\r\n    pub us_amount: f64,\r\n    pub cn_amount: f64,\r\n    pub counter_amount: f64,\r\n    pub details: Vec<BuyDetail>,\r\n    pub excluded: Vec<ExcludedFromBuy>,\r\n}\r\n\r\n#[derive(Debug)]\r\npub struct BuyDetail {\r\n    pub asset_code: String,\r\n    pub asset_name: String,\r\n    pub amount: f64,\r\n}\r\n\r\n#[derive(Debug)]\r\npub struct SellSuggestion {\r\n    pub asset_code: String,\r\n    pub asset_name: String,\r\n    pub annualized_return: Option<f64>,\r\n    pub absolute_return: f64,\r\n    pub sell_ratio: f64,\r\n    pub sell_shares: f64,\r\n    pub sell_amount: f64,\r\n    pub reason: SellReason,\r\n}\r\n\r\n#[derive(Debug)]\r\npub enum SellReason {\r\n    AnnualizedHigh,      // 年化收益达标\r\n    AbsoluteProfit,      // 绝对收益足够，长期持有获利了结\r\n}\r\n\r\n#[derive(Debug)]\r\npub struct RiskWarning {\r\n    pub asset_code: String,\r\n    pub asset_name: String,\r\n    pub loss_ratio: f64,\r\n    pub advice: RiskAdvice,\r\n}\r\n\r\n#[derive(Debug)]\r\npub enum RiskAdvice {\r\n    ConsiderBuyMore,  // 恐慌环境下浮亏，可能是加仓机会\r\n    ReviewFundamentals, // 中性环境下浮亏，审视基本面\r\n    UrgentReview,      // 贪婪环境下浮亏，需要紧急审视\r\n}\r\n\r\n/// 计算卖出建议后回收的现金总额\r\nfn total_sell_proceeds(suggestions: &[SellSuggestion]) -> f64 {\r\n    suggestions.iter().map(|s| s.sell_amount).sum()\r\n}\r\n\r\n/// 买入建议中标记因高浮亏被排除加仓的标的\r\n#[derive(Debug)]\r\npub struct ExcludedFromBuy {\r\n    pub asset_code: String,\r\n    pub asset_name: String,\r\n    pub loss_ratio: f64,\r\n    pub reason: String,\r\n}\r\n\r\n/// 计算买入建议\r\n/// sell_proceeds: 卖出建议预计回收的现金，用于实现买卖互感知\r\n/// risk_warnings: 风险警告列表，高浮亏标的将被排除加仓\r\npub fn calculate_buy_suggestions(\r\n    config: &AppConfig,\r\n    score: f64,\r\n    cash_balance: f64,\r\n    positions: &[Position],\r\n    sell_suggestions: &[SellSuggestion],\r\n    risk_warnings: &[RiskWarning],\r\n) -> BuySuggestion {\r\n    // 买入可用现金 = 当前现金 + 卖出回收\r\n    let sell_proceeds = total_sell_proceeds(sell_suggestions);\r\n    let available_cash = cash_balance + sell_proceeds;\r\n\r\n    let ratio = config.buy_ratio_for(score) / 100.0;\r\n    let total_amount = available_cash * ratio;\r\n\r\n    let us_ratio = config.allocation.us_stocks / 100.0;\r\n    let cn_ratio = config.allocation.cn_stocks / 100.0;\r\n    let cc_ratio = config.allocation.counter_cyclical / 100.0;\r\n\r\n    let us_amount = total_amount * us_ratio;\r\n    let cn_amount = total_amount * cn_ratio;\r\n    let counter_amount = total_amount * cc_ratio;\r\n\r\n    // 按逆向加权分配：浮亏越多获得越多资金\r\n    // 高浮亏标的（≥30%）排除加仓，避免\"越亏越买\"的风险\r\n    let excluded: Vec<ExcludedFromBuy> = risk_warnings\r\n        .iter()\r\n        .filter(|w| w.loss_ratio >= 30.0)\r\n        .map(|w| ExcludedFromBuy {\r\n            asset_code: w.asset_code.clone(),\r\n            asset_name: w.asset_name.clone(),\r\n            loss_ratio: w.loss_ratio,\r\n            reason: \"浮亏≥30%，暂停逆向加仓以防基本面恶化\".to_string(),\r\n        })\r\n        .collect();\r\n    let excluded_codes: Vec<String> = excluded.iter().map(|e| e.asset_code.clone()).collect();\r\n\r\n    let mut details = Vec::new();\r\n\r\n    let us_positions: Vec<&Position> = positions.iter().filter(|p| p.category == \"us_stocks\").collect();\r\n    let cn_positions: Vec<&Position> = positions.iter().filter(|p| p.category == \"cn_stocks\").collect();\r\n    let cc_positions: Vec<&Position> = positions.iter().filter(|p| p.category == \"counter_cyclical\").collect();\r\n\r\n    let max_weight = config.settings.max_contrarian_weight;\r\n\r\n    details.extend(distribute_amount_contrarian(&us_positions, us_amount, max_weight, &excluded_codes));\r\n    details.extend(distribute_amount_contrarian(&cn_positions, cn_amount, max_weight, &excluded_codes));\r\n    details.extend(distribute_amount_contrarian(&cc_positions, counter_amount, max_weight, &excluded_codes));\r\n\r\n    BuySuggestion {\r\n        total_amount,\r\n        us_amount,\r\n        cn_amount,\r\n        counter_amount,\r\n        details,\r\n        excluded,\r\n    }\r\n}\r\n\r\n/// 逆向加权分配：浮亏/低估的标的获得更多资金\r\n/// 权重 = min(max_weight, max(1.0, cost_price / current_price))，即浮亏越多权重越高但有上限\r\n/// 若所有持仓都浮盈，则等额分配\r\n/// excluded_codes: 因高浮亏被排除加仓的标的代码列表\r\nfn distribute_amount_contrarian(positions: &[&Position], total: f64, max_weight: f64, excluded_codes: &[String]) -> Vec<BuyDetail> {\r\n    if positions.is_empty() || total <= 0.0 {\r\n        return Vec::new();\r\n    }\r\n\r\n    // 过滤掉被排除的标的\r\n    let eligible: Vec<&&Position> = positions\r\n        .iter()\r\n        .filter(|p| !excluded_codes.contains(&p.asset_code))\r\n        .collect();\r\n\r\n    if eligible.is_empty() {\r\n        return Vec::new();\r\n    }\r\n    if eligible.len() == 1 {\r\n        return vec![BuyDetail {\r\n            asset_code: eligible[0].asset_code.clone(),\r\n            asset_name: eligible[0].asset_name.clone(),\r\n            amount: total,\r\n        }];\r\n    }\r\n\r\n    // 计算逆向权重：浮亏的标获得更高权重，但有上限防止过度集中\r\n    let weights: Vec<f64> = eligible\r\n        .iter()\r\n        .map(|p| {\r\n            match p.current_price {\r\n                Some(cur) if cur > 0.0 && p.cost_price > 0.0 => {\r\n                    // 浮亏时 cost/cur > 1，浮盈时 < 1，取 max(1.0, ...) 保证浮盈标的也有基础权重\r\n                    // 限制最大权重防止单标的过度集中\r\n                    (p.cost_price / cur).max(1.0).min(max_weight)\r\n                }\r\n                _ => 1.0, // 无现价时给予等额权重\r\n            }\r\n        })\r\n        .collect();\r\n\r\n    let total_weight: f64 = weights.iter().sum();\r\n    if total_weight <= 0.0 {\r\n        // 等额分配兜底\r\n        let per = total / eligible.len() as f64;\r\n        return eligible\r\n            .iter()\r\n            .map(|p| BuyDetail {\r\n                asset_code: p.asset_code.clone(),\r\n                asset_name: p.asset_name.clone(),\r\n                amount: per,\r\n            })\r\n            .collect();\r\n    }\r\n\r\n    eligible\r\n        .iter()\r\n        .zip(weights.iter())\r\n        .map(|(p, w)| BuyDetail {\r\n            asset_code: p.asset_code.clone(),\r\n            asset_name: p.asset_name.clone(),\r\n            amount: total * (w / total_weight),\r\n        })\r\n        .collect()\r\n}\r\n\r\n/// 计算卖出建议\r\n/// 改进：\r\n/// 1. 使用最小持仓天数门槛，避免短期年化失真触发卖出\r\n/// 2. 增加绝对收益考量：长期持有绝对收益超30%也可止盈\r\n/// 3. 中性区间按PRD矩阵补齐\r\npub fn calculate_sell_suggestions(\r\n    config: &AppConfig,\r\n    score: f64,\r\n    positions: &[Position],\r\n) -> Vec<SellSuggestion> {\r\n    let today = Local::now().date_naive();\r\n    let min_days = config.settings.min_holding_days;\r\n    let min_abs_days = config.settings.min_absolute_profit_days;\r\n    let mut suggestions = Vec::new();\r\n\r\n    for pos in positions {\r\n        if pos.shares <= 0.0 {\r\n            continue;\r\n        }\r\n        let current = match pos.current_price {\r\n            Some(p) if p > 0.0 => p,\r\n            _ => continue,\r\n        };\r\n\r\n        // 计算持仓天数\r\n        let holding_days = NaiveDate::parse_from_str(&pos.first_buy_date, \"%Y-%m-%d\")\r\n            .ok()\r\n            .map(|d| (today - d).num_days())\r\n            .unwrap_or(0);\r\n\r\n        // 年化收益（含最小天数门槛）\r\n        let ann_ret = pos.annualized_return_with_min_days(&today, min_days);\r\n\r\n        // 绝对收益（不受天数限制）\r\n        let abs_ret = pos.absolute_return().unwrap_or(0.0);\r\n\r\n        // 判断是否触发卖出\r\n        let (ratio, reason) = if let Some(ann) = ann_ret {\r\n            // 年化收益有效，按矩阵判断\r\n            let r = config.sell_ratio_for(score, ann * 100.0) / 100.0;\r\n            if r > 0.0 {\r\n                (r, SellReason::AnnualizedHigh)\r\n            } else if abs_ret >= 0.30 && holding_days >= min_abs_days {\r\n                // 年化不达标但绝对收益≥30%且持仓足够长（长期持有获利），在贪婪及以上环境减仓\r\n                if score >= config.thresholds.neutral {\r\n                    (0.20, SellReason::AbsoluteProfit)\r\n                } else {\r\n                    (0.0, SellReason::AnnualizedHigh) // 不触发\r\n                }\r\n            } else {\r\n                (0.0, SellReason::AnnualizedHigh) // 不触发\r\n            }\r\n        } else if abs_ret >= 0.30 && holding_days >= min_abs_days {\r\n            // 年化无效（持仓不足门槛天数），但绝对收益≥30%且持仓足够长\r\n            // 根据情绪区间差异化减仓：极度贪婪更多，中性较少\r\n            if score >= config.thresholds.greed {\r\n                (0.15, SellReason::AbsoluteProfit)\r\n            } else if score >= config.thresholds.neutral {\r\n                (0.10, SellReason::AbsoluteProfit)\r\n            } else {\r\n                continue;\r\n            }\r\n        } else {\r\n            continue;\r\n        };\r\n\r\n        if ratio > 0.0 {\r\n            let sell_shares = pos.shares * ratio;\r\n            let sell_amount = sell_shares * current;\r\n            suggestions.push(SellSuggestion {\r\n                asset_code: pos.asset_code.clone(),\r\n                asset_name: pos.asset_name.clone(),\r\n                annualized_return: ann_ret,\r\n                absolute_return: abs_ret,\r\n                sell_ratio: ratio * 100.0,\r\n                sell_shares,\r\n                sell_amount,\r\n                reason,\r\n            });\r\n        }\r\n    }\r\n    // 按绝对收益从高到低排序：优先卖出收益最高的标的以锁定利润\r\n    suggestions.sort_by(|a, b| b.absolute_return.partial_cmp(&a.absolute_return).unwrap_or(std::cmp::Ordering::Equal));\r\n    suggestions\r\n}\r\n\r\n/// 检查风险警告（浮亏超 20%）\r\n/// 改进：结合市场情绪给出差异化建议\r\npub fn check_risk_warnings(config: &AppConfig, score: f64, positions: &[Position]) -> Vec<RiskWarning> {\r\n    let mut warnings = Vec::new();\r\n    for pos in positions {\r\n        if pos.shares <= 0.0 || pos.cost_price <= 0.0 {\r\n            continue;\r\n        }\r\n        if let Some(current) = pos.current_price {\r\n            let ratio = current / pos.cost_price;\r\n            if ratio < 0.8 {\r\n                let advice = if score < config.thresholds.fear {\r\n                    // 恐慌环境下浮亏，可能是加仓机会\r\n                    RiskAdvice::ConsiderBuyMore\r\n                } else if score < config.thresholds.neutral {\r\n                    // 中性环境下浮亏，审视基本面\r\n                    RiskAdvice::ReviewFundamentals\r\n                } else {\r\n                    // 贪婪环境下浮亏，需要紧急审视（别人赚钱你还在亏）\r\n                    RiskAdvice::UrgentReview\r\n                };\r\n                warnings.push(RiskWarning {\r\n                    asset_code: pos.asset_code.clone(),\r\n                    asset_name: pos.asset_name.clone(),\r\n                    loss_ratio: (1.0 - ratio) * 100.0,\r\n                    advice,\r\n                });\r\n            }\r\n        }\r\n    }\r\n    warnings\r\n}\r\n"
    },
    "complexity_metrics": {
      "cyclomatic_complexity": 25.0,
      "lines_of_code": 314,
      "number_of_classes": 5,
      "number_of_functions": 5
    },
    "dependencies": [
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 1,
        "name": "AppConfig",
        "path": "src\\strategy.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": false,
        "line_number": 2,
        "name": "Position",
        "path": "src\\strategy.rs",
        "version": null
      },
      {
        "dependency_type": "use",
        "is_external": true,
        "line_number": 3,
        "name": "Local",
        "path": "src\\strategy.rs",
        "version": null
      }
    ],
    "detailed_description": "The strategy.rs component implements a sophisticated investment strategy engine that calculates buy and sell suggestions based on portfolio positions, market sentiment scores, and configurable rules. It consists of three main functions: calculate_buy_suggestions, calculate_sell_suggestions, and check_risk_warnings. The buy suggestion logic allocates available cash (current balance plus proceeds from sell suggestions) across US stocks, CN stocks, and counter-cyclical assets according to allocation ratios, then applies a contrarian weighting mechanism that prioritizes assets with higher unrealized losses (cost_price/current_price ratio), up to a configured maximum weight, while excluding assets with losses ≥30% to avoid 'buying into falling knives'. The sell suggestion logic evaluates each position for two triggers: high annualized return (adjusted by minimum holding days to avoid short-term noise) and absolute profit ≥30% with sufficient holding duration; it uses market sentiment score to dynamically adjust sell ratios, prioritizing higher absolute return positions for selling. The risk warning system identifies positions with >20% unrealized loss and provides context-sensitive advice (ConsiderBuyMore, ReviewFundamentals, UrgentReview) based on market sentiment levels. The component is tightly integrated with AppConfig for dynamic rule configuration and Position model for portfolio data, enabling a rules-based, sentiment-aware trading strategy that balances contrarian buying with profit-taking and risk management.",
    "interfaces": [
      {
        "description": null,
        "interface_type": "struct",
        "name": "BuySuggestion",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "BuyDetail",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "SellSuggestion",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "enum",
        "name": "SellReason",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "RiskWarning",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": null,
        "interface_type": "enum",
        "name": "RiskAdvice",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": "计算卖出建议后回收的现金总额",
        "interface_type": "function",
        "name": "total_sell_proceeds",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "suggestions",
            "param_type": "&[SellSuggestion]"
          }
        ],
        "return_type": "f64",
        "visibility": "private"
      },
      {
        "description": null,
        "interface_type": "struct",
        "name": "ExcludedFromBuy",
        "parameters": [],
        "return_type": null,
        "visibility": "public"
      },
      {
        "description": "逆向加权分配：浮亏/低估的标的获得更多资金 权重 = min(max_weight, max(1.0, cost_price / current_price))，即浮亏越多权重越高但有上限 若所有持仓都浮盈，则等额分配 excluded_codes: 因高浮亏被排除加仓的标的代码列表",
        "interface_type": "function",
        "name": "distribute_amount_contrarian",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "positions",
            "param_type": "&[&Position]"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "total",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "max_weight",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "excluded_codes",
            "param_type": "&[String]"
          }
        ],
        "return_type": "Vec<BuyDetail>",
        "visibility": "private"
      },
      {
        "description": "检查风险警告（浮亏超 20%） 改进：结合市场情绪给出差异化建议",
        "interface_type": "function",
        "name": "check_risk_warnings",
        "parameters": [
          {
            "description": null,
            "is_optional": false,
            "name": "config",
            "param_type": "&AppConfig"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "score",
            "param_type": "f64"
          },
          {
            "description": null,
            "is_optional": false,
            "name": "positions",
            "param_type": "&[Position]"
          }
        ],
        "return_type": "Vec<RiskWarning>",
        "visibility": "public"
      }
    ],
    "responsibilities": [
      "Calculate contrarian buy suggestions based on portfolio positions, available cash, and risk warnings, prioritizing undervalued assets while excluding severely underperforming ones",
      "Generate sell suggestions using dual triggers (annualized return and absolute profit) with dynamic sell ratios adjusted by market sentiment and holding period constraints",
      "Identify and classify risk warnings for positions with significant unrealized losses, providing sentiment-aware advice for potential actions",
      "Integrate sell proceeds into buy calculations to enable closed-loop portfolio rebalancing between selling and buying decisions",
      "Apply configurable thresholds and allocation rules from AppConfig to ensure strategy adaptability across different market conditions and user preferences"
    ]
  }
]
```

## Memory Storage Statistics

**Total Storage Size**: 370179 bytes

- **documentation**: 162541 bytes (43.9%)
- **preprocess**: 118897 bytes (32.1%)
- **studies_research**: 88709 bytes (24.0%)
- **timing**: 32 bytes (0.0%)

## Generated Documents Statistics

Number of Generated Documents: 12

- Key Modules and Components Research Report_核心策略引擎
- Key Modules and Components Research Report_数据模型与持久化
- Database Overview
- Key Modules and Components Research Report_外部数据获取
- Key Modules and Components Research Report_报告生成服务
- Project Overview
- Core Workflows
- Boundary Interfaces
- Key Modules and Components Research Report_命令行接口
- Key Modules and Components Research Report_系统入口
- Architecture Description
- Key Modules and Components Research Report_配置管理
