//! Workspace contracts for external ACP agents.
//!
//! The first prompt is a poor place for standing rules: it is long, it is
//! repeated every turn, and it drifts. External agents already discover a
//! project contract from the working directory (`AGENTS.md`) and load
//! task-specific knowledge from skills (`.agents/skills/<name>/SKILL.md`).
//! This module writes that contract once, into the directories MNS owns.
//!
//! Verified against `opencode acp` 2.2.5: it injects the cwd `AGENTS.md` into
//! the system prompt, and it discovers both `.agents/skills` and
//! `.claude/skills` (same-name entries are de-duplicated). ACP itself has no
//! stable system-prompt slot — `initialize` advertises no
//! `sessionCapabilities.systemPrompt` — so the file is the only reliable
//! carrier.
//!
//! Two rules keep this safe:
//!
//! * Only `~/.mns/agent/**` is touched. A user-chosen `working_dir` is their
//!   own repository; writing an MNS block there would collide with its rules.
//! * Only the managed block is rewritten. Everything outside
//!   `<!-- mns:begin v1 -->` … `<!-- mns:end -->` is user text and survives.
//!   Identical content is never written again, so mtimes stay stable.

use std::path::Path;

const BEGIN: &str = "<!-- mns:begin v1 -->";
const END: &str = "<!-- mns:end -->";

/// Which contract the workspace should carry. A butler conversation must not
/// inherit the task contract's `mns-decision` machine channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Profile {
    Task,
    Butler,
}

/// Absolute binaries to name in the contract. `None` is a real state: the
/// skill must say it is unavailable rather than print a fake path.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Binaries<'a> {
    pub mns: Option<&'a Path>,
    pub butler: Option<&'a Path>,
}

/// Write the managed contract into `cwd` when MNS owns that directory.
///
/// Returns whether a managed `AGENTS.md` is now present. `Ok(false)` means the
/// caller should keep the full rules in the prompt: either this is a
/// user-chosen directory, or writing failed and there is nothing to rely on.
pub(crate) fn bootstrap(cwd: &Path, profile: Profile, bins: Binaries<'_>) -> Result<bool, String> {
    bootstrap_under(
        &crate::config::app_data_dir().join("agent"),
        cwd,
        profile,
        bins,
    )
}

fn bootstrap_under(
    root: &Path,
    cwd: &Path,
    profile: Profile,
    bins: Binaries<'_>,
) -> Result<bool, String> {
    // `starts_with` is component-wise, so `~/.mns/agent-evil` does not pass.
    if !cwd.starts_with(root) {
        return Ok(false);
    }

    let agents = cwd.join("AGENTS.md");
    let existing = std::fs::read_to_string(&agents).unwrap_or_default();
    let merged = merge_managed(&existing, &managed_block(profile, bins));
    write_if_changed(&agents, &merged)
        .map_err(|error| format!("无法写入 {}：{error}", agents.display()))?;

    // `CLAUDE.md` is Claude Code's entry point; `GEMINI.md` is Gemini CLI's.
    // A one-line import keeps them on the same contract without duplicating it.
    // The import goes above whatever the user already wrote, so their notes
    // survive a bootstrap that runs on every session.
    for compat in ["CLAUDE.md", "GEMINI.md"] {
        let path = cwd.join(compat);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let merged = merge_compat_import(&existing);
        write_if_changed(&path, &merged)
            .map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
    }

    // Both profiles get the same two skills. The butler is no longer a
    // facts-only chatbot: it can query the account and turn a reminder into a
    // real `mns-butler task add`. A stale read-only copy from an older build
    // would contradict that, so it is overwritten by `install_skill`.
    install_skill(cwd, "mns-account", &account_skill(bins.mns))?;
    install_skill(cwd, "mns-butler", &butler_skill(bins.butler))?;

    Ok(true)
}

/// Prepend the managed import to a compatibility file, keeping user text.
///
/// `@AGENTS.md` is a one-line import understood by Claude Code and Gemini CLI.
/// An existing file may already contain it; in that case the text is returned
/// unchanged so repeated bootstraps stay a no-op.
fn merge_compat_import(existing: &str) -> String {
    if existing
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("@agents.md"))
    {
        return existing.to_owned();
    }
    let trimmed = existing.trim_end();
    if trimmed.is_empty() {
        "@AGENTS.md\n".to_owned()
    } else {
        format!("@AGENTS.md\n\n{trimmed}\n")
    }
}

fn install_skill(cwd: &Path, name: &str, body: &str) -> Result<(), String> {
    // opencode reads both locations and de-duplicates by skill name. Keeping
    // them in sync is cheap, and it covers Claude Code, which only reads
    // `.claude/skills`.
    for root in [".agents/skills", ".claude/skills"] {
        let path = cwd.join(root).join(name).join("SKILL.md");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建 {}：{error}", parent.display()))?;
        }
        write_if_changed(&path, body)
            .map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
    }
    Ok(())
}

/// Merge the managed block into existing text, preserving user content.
///
/// A complete block is replaced in place. A missing block is appended, so the
/// user's notes stay above it. A half-written block (begin without end) is not
/// trusted: the text is kept and a fresh block is appended.
fn merge_managed(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(BEGIN) {
        if let Some(end_offset) = existing[start..].find(END) {
            let end = start + end_offset + END.len();
            let mut merged = String::with_capacity(existing.len() + block.len());
            merged.push_str(&existing[..start]);
            merged.push_str(block);
            merged.push_str(&existing[end..]);
            return merged;
        }
    }

    let trimmed = existing.trim_end();
    if trimmed.is_empty() {
        format!("{block}\n")
    } else {
        format!("{trimmed}\n\n{block}\n")
    }
}

/// Write only when the bytes differ. Returns whether a write happened.
///
/// This is the whole mtime-stability contract: bootstrap runs on every
/// session, and rewriting an unchanged file would make every editor and
/// watcher think the workspace changed.
///
/// The write itself goes through a temp file in the same directory and a
/// rename, so a crash (or a full disk) cannot leave a half-written contract
/// behind — `AGENTS.md` is injected into every future system prompt.
fn write_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if let Ok(current) = std::fs::read_to_string(path) {
        if current == content {
            return Ok(false);
        }
    }

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("路径没有父目录：{}", path.display()),
        )
    })?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_owned());
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)?;
    Ok(true)
}

fn managed_block(profile: Profile, bins: Binaries<'_>) -> String {
    match profile {
        Profile::Task => task_block(bins),
        Profile::Butler => butler_block(bins),
    }
}

fn task_block(bins: Binaries<'_>) -> String {
    format!(
        "{BEGIN}\n\
         # MNS 任务工作区（由 MNS 自动维护，请勿手改本块；块外内容会保留）\n\n\
         你在 MNS 桌面端打开的工作区里执行用户交代的任务。\n\n\
         ## 数据入口\n\
         {mns_line}\n\
         - 账户、持仓、现金、绩效、定投与报告一律通过 mns CLI 读取。\n\
         - 不要用 find / mdfind / ls 搜索 mns，也不要依赖 PATH 上的同名命令。\n\n\
         ## 护栏（best-effort，不是沙箱）\n\
         - 使用 MNS 默认的 opencode 时，会改账本的 mns 子命令和 `init`\
         （buy / sell / cash set / today / report / update-prices 等）会被拒绝；\
         其余 shell、联网、编辑与子代理按 Agent 默认放行，其他子命令（含 `config params`）照常可用。\
         换其他 ACP agent 时这条护栏不生效。\n\
         - 禁止用 sqlite3 打开 `~/.mns/mns.db`。\n\
         - 禁止解析 `~/.mns/config.toml` 的内部结构。\n\
         - 禁止阅读 mns 源码。CLI 没有提供的数据，说明它不该由你获取。\n\n\
         ## 技能（相关时再加载）\n\
         - mns-account（账户与持仓）：`.agents/skills/mns-account/SKILL.md`\n\
         - mns-butler（管家状态与到点任务）：`.agents/skills/mns-butler/SKILL.md`\n\
         - 数字一律照抄命令输出，不要心算、四舍五入或凭记忆补。\n\n\
         ## 收尾\n\
         - 工作目录可写；但不要把结论另存成记录文件，最终结论按 prompt 的要求回传。\n\
         {END}",
        mns_line = mns_line(bins.mns),
    )
}

fn butler_block(bins: Binaries<'_>) -> String {
    format!(
        "{BEGIN}\n\
         # MNS 管家工作区（由 MNS 自动维护，请勿手改本块；块外内容会保留）\n\n\
         你是用户的私人投资管家，在 MNS 桌面端里与用户对话。\n\n\
         ## 数据入口\n\
         {mns_line}\n\
         - 账户事实已由 MNS 在每轮开场注入；够答就直接答，缺数据用 mns 只读子命令查，\
         查不到就直说没有。\n\
         - 管家状态、记忆、任务与未送达通知用 mns-butler 查。\n\
         {butler_line}\n\n\
         ## 护栏（best-effort，不是沙箱）\n\
         - 不要写任意文件；管家状态与到点任务只用 mns-butler 改，不要动账本。\
         使用 MNS 默认的 opencode 时，会改账本的 mns 子命令会被拒绝。\
         换其他 ACP agent 时这条护栏不生效。\n\
         - 不要用 sqlite3 打开 `~/.mns/mns.db`，也不要解析 `~/.mns/config.toml` 的内部结构。\n\n\
         ## 技能（相关时再加载）\n\
         - mns-account（账户与持仓）：`.agents/skills/mns-account/SKILL.md`\n\
         - mns-butler（管家状态与到点任务）：`.agents/skills/mns-butler/SKILL.md`\n\n\
         ## 说话与边界\n\
         - 不要输出 mns-decision、action、effective、monthly_amount 之类的机器字段。\n\
         - 不要谎称已下单、已记账或已设置提醒；提醒必须真的用 `mns-butler task add` 建了任务，\
         并且看到「已接受」才能说记下了。\n\
         - 到点任务只会送达一句话；`--talk` 也只一句问候。不要承诺「到点陪你聊两句」。\n\
         - 不要用 sleep / osascript / display notification 假装提醒。\n\
         - 「记住 / 忘掉」由管家界面解析并落盘，不要调用 remember / forget，也不要重复写入。\n\
         {END}",
        mns_line = mns_line(bins.mns),
        butler_line = butler_line(bins.butler),
    )
}

fn mns_line(mns: Option<&Path>) -> String {
    match mns {
        Some(path) => format!("- mns 绝对路径：{}\n  - 常用：{}", quoted(path), examples(path)),
        None => "- 未检测到随应用安装的 mns CLI。需要账户数据时停止并告知用户，不要搜索文件系统，也不要改用其他数据源。"
            .to_owned(),
    }
}

fn butler_line(butler: Option<&Path>) -> String {
    match butler {
        Some(path) => format!(
            "- mns-butler 绝对路径：{}\n  - 状态与任务：`status`、`memory`、`tasks`、`outbox`、\
             `task add \"<提醒什么>\" --in 5m [--user] [--talk]`（绝对时刻才用 `--at \"YYYY-MM-DD HH:MM[:SS]\"`）、\
             `task done <id>`",
            quoted(path)
        ),
        None => {
            "- 未检测到随应用安装的 mns-butler。无法查询或创建到点任务，请如实告知用户，不要猜测。"
                .to_owned()
        }
    }
}

fn examples(mns: &Path) -> String {
    let mns = quoted(mns);
    format!(
        "`{mns} portfolio`、`{mns} cash`、`{mns} performance`、\
         `{mns} plan --monthly <金额>`、`{mns} guard`"
    )
}

fn account_skill(mns: Option<&Path>) -> String {
    let body = match mns {
        Some(mns) => {
            let mns = quoted(mns);
            format!(
                "- `{mns} portfolio` 持仓、成本与市值\n\
                 - `{mns} cash` 现金与流水\n\
                 - `{mns} performance` 实际 XIRR 与回撤\n\
                 - `{mns} plan --monthly <金额>` 定投分配\n\
                 - `{mns} guard` 买入冷静期\n\n\
                 只读查询只用上面这几条。需要刷新价格或生成报告时，告知用户自行运行外部命令；\n\
                 这些写操作不在你的权限范围内。"
            )
        }
        None => "当前未检测到 mns CLI，本技能不可用。需要账户数据时直接告知用户，不要搜索文件系统或猜测数字。".to_owned(),
    };
    format!(
        "---\n\
         name: mns-account\n\
         description: 只读查询 MNS 账户数据（持仓、现金、绩效、定投计划、买入冷静期）。用户问仓位、收益、什么时候买时使用。\n\
         ---\n\n\
         # mns-account\n\n\
         只读技能。所有数字必须照抄命令输出，不要心算、四舍五入或凭记忆补全。\n\n\
         ## 命令（绝对路径）\n\
         {body}\n\n\
         ## 禁止\n\
         - 不要用 sqlite3 打开 `~/.mns/mns.db`。\n\
         - 不要解析 `~/.mns/config.toml` 的内部结构。\n\
         - 不要阅读 mns 源码，也不要用 find / mdfind / ls 搜索 mns。\n\
         - CLI 没有提供的数据，说明它不该由你获取。\n"
    )
}

fn butler_skill(butler: Option<&Path>) -> String {
    let body = match butler {
        Some(butler) => {
            let butler = quoted(butler);
            format!(
                "- `{butler} status` 唤醒状态、开关、记忆与任务概览\n\
                 - `{butler} memory` 常驻记忆（`--archived` 看归档）\n\
                 - `{butler} tasks` 待执行与已归档任务\n\
                 - `{butler} outbox` 最近已授权但未送达的通知\n\
                 - `{butler} task add \"<提醒什么>\" --in 5m [--user] [--talk]` 建一条到点任务\
                 （绝对时刻才用 `--at \"YYYY-MM-DD HH:MM[:SS]\"`）\n\
                 - `{butler} task done <id>` 标记任务完成\n\
                 - `{butler} wake` 手动巡检；`{butler} mute` / `{butler} unmute` 开关主动开口"
            )
        }
        None => "当前未检测到 mns-butler，本技能不可用。不要用其他命令替代，也不要猜测管家状态。"
            .to_owned(),
    };
    format!(
        "---\n\
         name: mns-butler\n\
         description: 受控读写 MNS 管家状态与到点任务（查看状态/记忆/任务/出箱，创建到点检查任务、标记完成）。用户要提醒、问管家记得什么、问待办时使用。\n\
         ---\n\n\
         # mns-butler\n\n\
         受控读写技能。状态与数字照抄命令输出；命令失败就如实转述，不要假装成功。\n\n\
         ## 命令（绝对路径）\n\
         {body}\n\n\
         ## 建提醒\n\
         1. 相对时间用 `--in 90s` / `--in 5m` / `--in 2h` / `--in 1d`；不要自己算绝对时刻，\
         也不要先 `date` 再心算加几分钟——分钟截断会让「1 分钟后」变成几秒后。\n\
         2. 只有用户给的是绝对时刻才用 `--at`，格式 `%Y-%m-%d %H:%M` 或 `%Y-%m-%d %H:%M:%S`（本地时间）。\n\
         3. 用户亲口要的提醒加 `--user`：不要求写成「检查什么」，也不占管家的每日 2 条额度；\
         但不能含 预计 / 预测 / 会涨 / 会跌 / 抄底 / 加仓到。\n\
         4. 管家自己提议的检查任务不加 `--user`：意图必须含 检查 / 核对 / 确认 / 看 / 复盘 之一，\
         每天最多 2 条。活跃任务最多 10 条；默认键是「意图前 24 字 @ 到点时刻」，\
         同一句话在不同时间可以各建一条。\n\
         5. 用户原话里有「打个招呼 / 主动开口 / 陪我聊两句」就加 `--talk`：到点送一句问候，\
         不回显任务原文。但这也只是一句话，不会自动展开长对话。\n\
         6. 只有看到「已接受任务 <id>，到点 ...」才能回执；看到「未接受：...」就把原因如实转述给用户。\
         到点时间已经过去会被拒绝——改用 `--in`，不要把过去的时刻硬塞进来。\n\n\
         ## 回执口径\n\
         - 能提醒时：「记在 <时间>，到点我提醒你；那会儿 MNS 要开着（托盘也算），\
         没开就只会在你下次打开时补上（24 小时内）。」\n\
         - 不要承诺「一定弹」；说清到点时刻与前提即可。\n\
         - 你亲口设的到点任务不受主动开口开关、频控和静音影响；应用完全退出时\
         通知留在 `pending.jsonl`，下次连上补，但只在 24 小时内成立。\n\
         - 到点后 24 小时仍未处理的任务会作废。\n\n\
         ## 不要做\n\
         - 不要调用 `remember` / `forget`：GUI 在消息进入 agent 之前已把「记住…/忘掉…」\
         本地落盘，再写一次就是重复。\n\
         - 不要用 `sleep` / `osascript` / `display notification` 假装提醒。\n\
         - 不要写任意文件、不要动账本；管家状态与到点任务只用本技能里的 mns-butler 命令改。\n"
    )
}

/// Quote a path for a shell so spaces survive. Paths from the bundle contain
/// spaces on Windows, and an unquoted example is an invitation to split it.
fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn bin() -> PathBuf {
        PathBuf::from("/Applications/MNS.app/Contents/MacOS/mns")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mns-workspace-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn merge_keeps_user_content_and_is_idempotent() {
        let block = managed_block(
            Profile::Task,
            Binaries {
                mns: Some(&bin()),
                butler: None,
            },
        );
        let original = "# 我的笔记\n\n只在周末复盘。";
        let once = merge_managed(original, &block);
        assert!(
            once.starts_with(original),
            "user text above the block survives"
        );
        assert!(once.contains(&block));
        let twice = merge_managed(&once, &block);
        assert_eq!(once, twice, "a second bootstrap must be a no-op");
    }

    #[test]
    fn merge_replaces_only_the_managed_block() {
        let old = format!("前言\n{BEGIN}\n旧内容\n{END}\n后记\n");
        let block = managed_block(
            Profile::Butler,
            Binaries {
                mns: Some(&bin()),
                butler: None,
            },
        );
        let merged = merge_managed(&old, &block);
        assert!(merged.starts_with("前言\n"));
        assert!(merged.ends_with("\n后记\n"));
        assert!(!merged.contains("旧内容"), "the stale block is gone");
        assert!(merged.contains("MNS 管家工作区"));
    }

    #[test]
    fn install_path_change_rewrites_the_file() {
        let dir = scratch("rewrite");
        let path = dir.join("AGENTS.md");
        let first = managed_block(
            Profile::Task,
            Binaries {
                mns: Some(Path::new("/old/mns")),
                butler: None,
            },
        );
        assert!(write_if_changed(&path, &first).unwrap());
        assert!(
            !write_if_changed(&path, &first).unwrap(),
            "same bytes: no rewrite"
        );

        let second = managed_block(
            Profile::Task,
            Binaries {
                mns: Some(&bin()),
                butler: None,
            },
        );
        assert!(
            write_if_changed(&path, &second).unwrap(),
            "changed path must rewrite"
        );
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(stored.contains("/Applications/MNS.app/Contents/MacOS/mns"));
        assert!(!stored.contains("/old/mns"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_writes_contract_skills_and_compat_imports() {
        let dir = scratch("bootstrap");
        // Simulate an MNS-managed directory under a scratch root.
        let root = dir.join("agent");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();

        assert!(
            bootstrap_under(
                &root,
                &cwd,
                Profile::Task,
                Binaries {
                    mns: Some(&bin()),
                    butler: Some(Path::new("/bundle/mns-butler"))
                },
            )
            .unwrap()
        );

        let agents = std::fs::read_to_string(cwd.join("AGENTS.md")).unwrap();
        assert!(agents.contains("MNS 任务工作区"));
        assert!(agents.contains("/Applications/MNS.app/Contents/MacOS/mns"));
        assert!(agents.contains(BEGIN) && agents.contains(END));
        assert_eq!(
            std::fs::read_to_string(cwd.join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("GEMINI.md")).unwrap(),
            "@AGENTS.md\n"
        );

        let account =
            std::fs::read_to_string(cwd.join(".agents/skills/mns-account/SKILL.md")).unwrap();
        assert!(account.contains("name: mns-account"));
        assert!(account.contains("/Applications/MNS.app/Contents/MacOS/mns"));
        let claude_account =
            std::fs::read_to_string(cwd.join(".claude/skills/mns-account/SKILL.md")).unwrap();
        assert_eq!(
            account, claude_account,
            "both discovery paths carry the same body"
        );

        let butler =
            std::fs::read_to_string(cwd.join(".agents/skills/mns-butler/SKILL.md")).unwrap();
        assert!(butler.contains("name: mns-butler"));
        assert!(butler.contains("/bundle/mns-butler"));
        assert!(butler.contains("task add"));
        assert!(butler.contains("MNS 要开着"));
        let claude_butler =
            std::fs::read_to_string(cwd.join(".claude/skills/mns-butler/SKILL.md")).unwrap();
        assert_eq!(
            butler, claude_butler,
            "frontmatter and body must stay byte-identical across both paths"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_never_touches_a_user_working_dir() {
        let dir = scratch("custom");
        let root = dir.join("agent");
        let cwd = dir.join("my-repo");
        std::fs::create_dir_all(&cwd).unwrap();

        assert!(
            !bootstrap_under(
                &root,
                &cwd,
                Profile::Task,
                Binaries {
                    mns: Some(&bin()),
                    butler: None
                },
            )
            .unwrap()
        );
        assert!(
            !cwd.join("AGENTS.md").exists(),
            "a user repo must not be polluted"
        );
        assert!(!cwd.join(".agents").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_compat_file_gets_the_import_above_user_text() {
        let dir = scratch("compat");
        let root = dir.join("agent");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let claude = cwd.join("CLAUDE.md");
        std::fs::write(&claude, "# 用户自己的 Claude 约定\n").unwrap();

        bootstrap_under(&root, &cwd, Profile::Task, Binaries::default()).unwrap();
        let merged = std::fs::read_to_string(&claude).unwrap();
        assert_eq!(
            merged, "@AGENTS.md\n\n# 用户自己的 Claude 约定\n",
            "the import is prepended, user text survives"
        );

        // A second bootstrap must be a no-op.
        bootstrap_under(&root, &cwd, Profile::Task, Binaries::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), merged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn butler_bootstrap_installs_skills_and_names_the_binaries() {
        let dir = scratch("butler");
        let root = dir.join("agent");
        let cwd = root.join("butler");
        std::fs::create_dir_all(&cwd).unwrap();
        // A stale read-only copy from an older build must be overwritten.
        for name in ["mns-account", "mns-butler"] {
            let skill = cwd.join(".agents/skills").join(name).join("SKILL.md");
            std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
            std::fs::write(skill, "stale").unwrap();
        }

        assert!(
            bootstrap_under(
                &root,
                &cwd,
                Profile::Butler,
                Binaries {
                    mns: Some(&bin()),
                    butler: Some(Path::new("/bundle/mns-butler")),
                },
            )
            .unwrap()
        );

        let agents = std::fs::read_to_string(cwd.join("AGENTS.md")).unwrap();
        assert!(agents.contains("MNS 管家工作区"));
        assert!(agents.contains("/Applications/MNS.app/Contents/MacOS/mns"));
        assert!(agents.contains("/bundle/mns-butler"));
        assert!(agents.contains("mns-account") && agents.contains("mns-butler"));
        assert!(agents.contains("best-effort"));
        assert!(!agents.contains("不要执行命令"));
        assert!(
            agents.contains("task add"),
            "the butler contract must name the real reminder path"
        );

        for root in [".agents/skills", ".claude/skills"] {
            let account = std::fs::read_to_string(cwd.join(root).join("mns-account/SKILL.md"))
                .expect("account skill is installed for the butler");
            assert!(account.contains("name: mns-account"));
            let butler = std::fs::read_to_string(cwd.join(root).join("mns-butler/SKILL.md"))
                .expect("butler skill is installed for the butler");
            assert!(butler.contains("task add"));
            assert!(!butler.contains("stale"));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binaries_are_reported_instead_of_faked() {
        let block = managed_block(Profile::Task, Binaries::default());
        assert!(block.contains("未检测到随应用安装的 mns CLI"));
        assert!(!block.contains("None"));

        let account = account_skill(None);
        assert!(account.contains("本技能不可用"));
        assert!(
            !account.contains("portfolio`"),
            "no example may imply a working CLI"
        );

        let butler = butler_skill(None);
        assert!(butler.contains("本技能不可用"));
        assert!(!butler.contains("/bundle/mns-butler"));
    }

    #[test]
    fn task_and_butler_contracts_do_not_leak_into_each_other() {
        let task = managed_block(
            Profile::Task,
            Binaries {
                mns: Some(&bin()),
                butler: None,
            },
        );
        let butler = managed_block(
            Profile::Butler,
            Binaries {
                mns: Some(&bin()),
                butler: None,
            },
        );

        assert!(task.contains("MNS 任务工作区"));
        assert!(
            !task.contains("不要输出 mns-decision"),
            "a task run owns the machine channel"
        );

        assert!(butler.contains("MNS 管家工作区"));
        assert!(
            butler.contains("不要输出 mns-decision"),
            "a butler chat must not emit the block"
        );
        assert!(butler.contains("不要谎称"));

        // Both profiles name the same skills on demand. Capability comes
        // first; the only MNS-specific restriction is the ledger guardrail.
        for contract in [&task, &butler] {
            assert!(contract.contains("mns-account"));
            assert!(contract.contains("mns-butler"));
            assert!(contract.contains("相关时再加载"));
            assert!(contract.contains("best-effort"));
        }
        assert!(
            task.contains("工作目录可写") && !task.contains("工作目录只读"),
            "the task workspace is writable again"
        );
        assert!(
            task.contains("buy / sell / cash set"),
            "the guard is named explicitly instead of denying the whole shell"
        );
        assert!(
            task.contains("config params") && !task.contains("init / config"),
            "a read-only mns config subcommand must not be caught by the guard"
        );
        assert!(
            butler.contains("不要写任意文件") && butler.contains("只用 mns-butler 改"),
            "the butler may write its own state through mns-butler, not arbitrary files"
        );
    }

    #[test]
    fn skill_bodies_name_the_real_installed_path() {
        let mns = bin();
        let butler = PathBuf::from("/bundle/mns-butler");
        let account = account_skill(Some(&mns));
        assert!(
            account.contains(&format!("\"{}\"", mns.display())),
            "the rendered path must match the binary the session was given"
        );
        let butler = butler_skill(Some(&butler));
        assert!(butler.contains("\"/bundle/mns-butler\""));
        assert!(butler.contains("status"));
    }
}
