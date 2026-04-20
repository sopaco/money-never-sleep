# @never-sleeps/mns-cli

MNS (Money Never Sleep) CLI 安装包。通过 npm optional dependencies 机制，根据当前平台自动安装对应的预编译 binary。

## 安装

```bash
npm install -g @never-sleeps/mns-cli
```

安装后即可直接使用 `mns` 命令：

```bash
mns init
mns portfolio
mns report
```

## 支持平台

| 平台 | 包名 |
|------|------|
| macOS Apple Silicon | `@never-sleeps/bin-darwin-arm64` |
| Linux x64 | `@never-sleeps/bin-linux-x64` |
| Windows x64 | `@never-sleeps/bin-win-x64` |

npm 会根据 `os`/`cpu` 字段自动选择并安装当前平台对应的 binary，无需手动判断平台。

## 相关项目

- [money-never-sleep](https://github.com/sopaco/money-never-sleep) — MNS CLI 源码
- Agent Skill: `@never-sleeps/skill-mns-cli`
