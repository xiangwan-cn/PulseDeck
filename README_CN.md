# PulseDeck

简体中文 | [English](README.md)

PulseDeck 是面向 Linux 手机、平板和桌面的轻量 GTK4/Libadwaita 配置化仪表盘。
页面、指标卡片、刷新计划、解析器和操作均可用 TOML 或 JSON 描述，因此大多数界面
调整无需重新编译应用。

![PulseDeck 默认仪表盘](docs/images/pulsedeck-default.png)

_未启用任何可选 Cargo feature 的暗色默认构建：左侧为标准布局，右侧为紧凑布局。_

## 功能

- 原生支持 CPU、内存、电池、功耗、网络、运行时间、文件系统、进程数、负载、
  交换空间、温度及网络吞吐指标。
- 支持内置指标、文件、命令、HTTP 和静态值数据源。
- 支持数值、进度、状态、文本、列表、组合和操作渲染器。
- 普通卡片支持有序视觉状态规则，可按数值、文本或数据源状态匹配，并覆盖文案、图标、
  分区颜色、多色背景和不增加定时器的颜色过渡。
- 主数值统一使用直观格式：整百分比不显示无意义小数，单位自然排版，网络卡优先显示
  IP，功耗卡优先显示功率。
- 支持固定间隔或 `daily@08:00,20:00` 等时间计划，并按时间槽缓存。
- 全局及单卡片响应式尺寸，适配移动端和桌面布局。
- 页面不可见时停止轮询。
- 使用纯事实到策略模型，分别管理已映射／活动／空闲状态、工作与视觉级别、屏幕抑制、
  Agent 通知和可选的应用内空闲视图，设置即时生效。
- 文件和网络状态事件驱动更新，临近刷新合并唤醒，共享系统快照并去重持久缓存写入。
- 限制子进程输出、HTTP 响应大小和执行时间。
- 可选、独立编译的 ScrcpyForge 设备控制页面。
- 可选、独立编译的 Codex/OpenCode/pi PetCard，支持事件驱动生命周期动画、展示尺寸
  记忆和完成提示音。
- 页面工具栏可在配置的普通网格与六列紧凑网格间切换，并跨启动记忆上次选择。

## 运行与低功耗策略

PulseDeck 将运行事实与纯策略计算分开。策略快照分别表达窗口可见性、用户活动、工作级别、
视觉级别、屏幕抑制、空闲视图、供电、温度和 Agent 状态。已映射但失去键盘焦点的窗口
因此不会被误判为后台。

只有点击／触摸、按键、滚动、拖动、切换页面、手动刷新、对话框响应和插件控制等真实
输入会重置空闲时间；自动刷新、动画、文件事件、Agent hook 和网络响应都不会重置。

| 状态 | B2 默认行为 |
| --- | --- |
| 已映射且活动 | 便宜监视工作按基础间隔运行；继承的高成本命令/HTTP 使用 30 分钟下限。 |
| 已映射但非活动或本地空闲 | 白天仍保持相同的完整工作；焦点、空闲和宽限仅用于诊断/界面。 |
| 夜间静默 | 按本地时钟暂停普通周期本地/远程工作；电源、电池、温度、网络、Agent 信号和显式一次性请求仍可用。 |
| 未映射 | 暂停普通工作和插件轮询；排队的手动/事件请求等重新映射后执行。 |

`profile = "performance" | "balanced" | "eco"` 仍是严格 v4 兼容/诊断字段，不是有效的设置页控制；
已映射白天默认工作均为完整策略。自动识别的高成本命令/HTTP 至少间隔 30 分钟；确实便宜的来源可显式标为
`normal`／`live`。显式单卡 `inactive_behavior`／`idle_behavior` 仍属于用户主动覆盖。
`screen_inhibit = "never" | "while-active" | "while-mapped"` 与刷新独立，只请求抑制
空闲熄屏，不阻止系统休眠。`idle_view = "none" | "dim" | "minimal"` 只影响
PulseDeck，不修改系统亮度。静默按本地时钟使用半开区间 `[start,end)`，起止小时相同则禁用；
手动/事件请求仍可执行。

外接电源不会清除 Low/Critical 电池阶段，也不会让工作超过白天的 Full；`external_boost`
仅保留为严格 v4 兼容/诊断字段，不是有效的设置页控制，默认关闭。电池低电量滞回默认是 20/25%，临界电量滞回是
10/15%。供电/温度信号回退保持有界（供电 15–300 秒、温度 15–60 秒）。观察租约默认 300 秒，
在夜间静默中临时恢复到期的监视工作，并且只由真实映射窗口输入续期；Agent、网络、文件、动画和自动
刷新事件都不会续期。温热只用于诊断；Hot/Throttled 独立限制工作并冻结 PetCard。
Agent 状态可驱动 PetCard 和去重通知，但不能让普通卡片、全局亮度或屏幕抑制保持满档。
完整策略、缓存、动作失效和有界恢复见 [docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md)。

## 页面布局模式

页面工具栏右侧的网格按钮控制通用指标卡和操作卡布局：

| 布局 | 行为 |
| --- | --- |
| 普通 | 使用 `[ui].card_columns`（默认三列）；卡片宽度均分整行，高度按页面可见区自适应为三行。 |
| 紧凑 | 将指标卡与操作卡重排为六列，同时仍以三行填满可见区，并使用更紧凑的间距、字号和控件。 |

工具栏选择保存在
`${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/compact-grid`，下次启动时自动恢复。
这是页面网格偏好，不是 PetCard 专属尺寸。切换网格会立即重排已经放大的 PetCard；
PetCard 自己的普通、占四格、占六格和全屏偏好见下文。全局与单卡片
`card_height` 仍作为小窗口或特意加高卡片的最小高度，显式宽度配置也继续有效。

## 环境要求

- 安装 GTK 4.10 或更高版本、Libadwaita 1.2 或更高版本的 Linux。
- Rust stable 及 GTK Rust 绑定所需的本机构建依赖。
- 自定义配置所引用的可选命令或服务。

Debian 系发行版常用开发包名称为 `libgtk-4-dev`、`libadwaita-1-dev`、
`pkg-config` 和 `build-essential`；其他发行版的软件包名称可能不同。

## 构建与运行

```sh
git clone https://github.com/xiangwan-cn/PulseDeck.git
cd PulseDeck
cargo build --release
./target/release/pulsedeck
```

如需包含可选 ScrcpyForge 页面：

```sh
cargo build --release --features scrcpy-forge
```

如需包含 PetCard，或同时包含两个可选集成：

```sh
cargo build --release --features pet-card
cargo build --release --features scrcpy-forge,pet-card
```

需要在实际功耗测试中查看应用内部唤醒与 I/O 计数时，可单独启用：

```sh
cargo build --release --features power-debug
```

## 配置

首次启动时，PulseDeck 会将内置示例复制到：

```text
${XDG_CONFIG_HOME:-$HOME/.config}/pulsedeck/config.toml
```

PulseDeck 还会自动扫描同级的 `config.d/` 目录。该目录第一层的每个 `.toml` 或 `.json`
文件都是独立模块，可以包含页面、卡片、操作或显式命名覆盖；文件按名称字典序加载，
子目录与其他扩展名会被忽略。因此导出卡片或页面只需复制一个文件，不需要维护 include
列表；把扩展名改为 `.disabled` 即可停用模块。

配置采用严格 schema v4，正常启动绝不会自动迁移。文件根部必须包含
`schema_version = 4`；未知字段、废弃别名和未知枚举值会使配置加载失败，而不是被静默
忽略。v3 文件需显式运行 `pulsedeck config migrate`。

建议从 [config/config.example.toml](config/config.example.toml) 开始。仓库同时提供
内容一致的 [config/config.example.json](config/config.example.json)。当前 TOML schema
及实用卡片示例见 [config/CARD_GUIDE.md](config/CARD_GUIDE.md)。
PetCard 的构建、hook、动画、尺寸、功耗和提示音行为见
[docs/PET_CARD.md](docs/PET_CARD.md)。
统一运行策略、调度行为、插件适配和功耗验证方法见
[docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md)。

顶层配置包括：

- `schema_version`：必填的配置接口版本，当前为 `4`。
- `[app]`：标题、日志、输出限制和配置重载。
- `[runtime]`：兼容诊断字段（`profile`、`external_boost`）、非活动／空闲行为、屏幕抑制、空闲视图、夜间静默、观察租约、供电／温度采样、电池滞回和 Agent 通知。
- `[ui]`：默认页面、普通网格列数和卡片尺寸；工具栏普通/紧凑选择作为 UI 状态单独保存。
- `[[pages]]`：按顺序排列的导航页面。
- `[[cards]]`：由可配置数据源提供内容的卡片。
- `[[actions]]`：用户主动触发的命令，可要求二次确认。

模块同样以 schema 版本开头，并可提供便于识别的名称：

```toml
schema_version = 4
name = "workstation"

[[cards]]
# ...一个或多个完整卡片...
```

默认情况下重复 ID 会直接报错。明确的个人覆盖模块可设置
`replace_existing = true`，从而替换更早文件中的同 ID 页面、卡片或操作，也可以按字段
覆盖 `[app]`、`[ui]` 或 `[runtime]`。未写字段继续继承主配置；设置页会把发生变化的字段
写回最后拥有该段的模块，因此默认
`config.toml` 不会被个人配置改写。可直接复制
[独立模块示例](config/config.d/50-custom.example.toml)。

可在不打开界面的情况下校验、格式化或生成配置。`add` 未指定 `--module` 时会列出已有
配置文件，并提供新建文件选项；显式指定时既可选择已有文件，也可创建新名称。已有文件
保持自己的覆盖属性，新建个人文件自动设置 `replace_existing = true`。因此个人覆盖文件中
与默认配置同 ID 的定义始终优先，同时工具不会硬编码任何用户专属文件名：

```sh
pulsedeck config check
pulsedeck config check /path/to/config.toml
pulsedeck config migrate # 显式执行 v3 -> v4，并创建 .v3.bak 备份
pulsedeck config add builtin cpu --id cpu-personal --title "CPU" --renderer progress --refresh 5s
pulsedeck config add command --id kernel --title "内核" --renderer text --refresh 1h --module 50-workstation.toml -- uname -r
pulsedeck config format # 规范化主文件及模块；会移除注释
```

最小自定义卡片示例：

```toml
[[cards]]
id = "kernel"
title = "内核"
page = "monitor"
renderer = "text"
refresh = "1h"
source = { command = { run = ["uname", "-r"], timeout = "5s" } }
```

普通非插件卡片还可从当前值推导命名视觉状态。首条匹配的
`[[cards.display.states]]` 规则可以覆盖文案、图标、强调边、主值、进度条和背景颜色；
`background` 数组会生成克制的多色渐变，`[cards.display.transition]` 则在不增加轮询或
动画定时器的前提下平滑切换状态。数值、文本、正则、语义级别和数据源生命周期匹配方式
见卡片配置指南。普通卡片还可配置静态本地 `[cards.display.background_svg]`，并用
`[cards.display.logo_svg]` 替换右上角刷新图标；Logo 不影响标题居中，在紧凑模式则作为
不可点击的装饰层保留。两种 SVG 都能与状态颜色共存且不产生额外刷新任务。

当 `reload_on_change = true` 时，主文件与启用模块的数值类修改都会在运行期间重新读取。新增或删除页面、
卡片后应重新打开应用，以重建完整页面结构。

## 数据源与渲染器

| 数据源写法 | 用途 |
| --- | --- | --- |
| `{ builtin = "cpu" }` | 高效读取 Linux 原生系统指标。 |
| `{ file = { path = "/path" } }` | 读取文本、sysfs 或 procfs 文件。 |
| `{ command = { run = ["program", "arg"] } }` | 不经过 shell，运行有边界的子进程。 |
| `{ http = { url = "https://…" } }` | 获取本地或远程数据。 |
| `{ text = "固定内容" }` | 标签和固定信息卡片。 |

渲染器包括 `value`、`progress`、`status`、`text`、`list`、`composite` 和
`action`。应选择与数据源输出匹配的渲染器；内置指标已经返回相应结构化数值。

## 可选 ScrcpyForge 集成

默认构建不包含该集成。启用 `scrcpy-forge` feature 后，如果配置中还没有同名页面，
PulseDeck 会自动创建 `config.d/90-scrcpy-forge.toml`；未编译该 feature 时绝不会复制
此文件，已有配置也不会被覆盖。`src/plugins/scrcpy_forge/config.example.toml` 是可直接
复制到 `config.d/` 的完整独立模块，仅用于显式自定义默认值。
它连接到单独安装的 ScrcpyForge 后端；PulseDeck 不持有 ADB 或 scrcpy 进程。服务
程序、URL 和脚本均可配置。预览与健康检查在插件内映射通用工作级别：

- `Full` 使用配置的预览间隔。
- `Reduced` 降低预览与健康检查频率。
- `Minimal` 保留轻量设备与脚本元数据，但不请求预览帧。
- `Suspended` 或页面隐藏时停止预览工作，不继续轮询。
- 未变化的画面继续通过 ETag／内容哈希缓存复用。

ScrcpyForge（简称 SF）是基于 ADB 与 scrcpy 的多设备 Android 自动化项目，提供设备
控制、画面预览与脚本自动化能力。项目介绍与使用说明见
[ScrcpyForge 项目主页](https://github.com/xiangwan-cn/ScrcpyForge)。

## 可选 Codex/OpenCode/pi PetCard

`pet-card` feature 通过通用卡片插件接口接入，Agent 专属状态和定时器不会进入主线
核心。`integrations/pulsedeck-pet` 中可单独安装的 Codex hook、OpenCode 插件和 pi
扩展只通过原子状态文件发布固定生命周期状态，不读取提示词、消息或工具内容。

使用 `--features pet-card` 编译后，如果配置中还没有 `codex-pet`，PulseDeck 会自动
创建并启用 `config.d/80-pet-card.toml`；未编译该 feature 时不会复制此模块。零配置
回退仍然可用，自定义帧路径则保持在独立模块内。

以下展示行为仅对 PetCard 有效：

- 双击依次切换普通、占四格、占六格和全屏；长按可打开菜单直接选择。

| PetCard 展示方式 | 行为 |
| --- | --- |
| 普通 | 保持在 FlowBox 原来的一个格内。 |
| 占四格 | 占据左侧两列、两个逻辑行，其余卡片填充右侧列。 |
| 占六格 | 占据左侧两列、三个逻辑行。 |
| 全屏 | 填满工具栏下方的当前页面，使用 `Escape` 或恢复按钮返回网格。 |

- 手动选择保存在 `config.toml` 之外的
  `${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/pet-card-presentation`。
  之后进入 `thinking`、`working`、`coding`、`waiting` 等任一活跃状态时，会自动恢复
  上次手动选择的展示尺寸。
- 连续离线达到 `offline_normal_after_seconds`（默认五分钟）后，PetCard 会临时缩回
  一个普通格。离线回退不会覆盖已保存的尺寸，下次进入活跃状态时会再次恢复。
- 占四格和占六格会跟随当前三列或六列页面网格重排，因此切换工具栏布局时周围卡片
  会立即重新排列。

PetCard 同样遵循通用视觉策略：窗口已映射时，无论活动还是非活动，活跃 Agent 状态在白天或
夜间观察租约内都使用配置的动画速率（最高 12 FPS）。没有观察租约时，夜间静默冻结所有循环动画，
但继续接收 Agent 状态；持续循环的非 Agent 状态白天限制为 1 FPS。完成／错误等有限动画可作为事件确认播放一次。Hot/Throttled
冻结当前帧，Warm/Unknown 不降速；卡片隐藏或应用后台时移除帧定时器，离线及单帧状态没有
动画定时器。Agent 动画不会提升普通刷新、远程工作、亮度或屏幕抑制。完成提示音由全局 Agent
通知设置控制。详见 [docs/PET_CARD.md](docs/PET_CARD.md)。

![PetCard 正在工作并占四格展示](docs/images/pulsedeck-petcard-working.png)

_暗色完整仪表盘：PetCard 处于工作状态，并使用占四格展示方式。_

## 项目结构

- `src/core`：配置、运行/供电状态、调度、缓存和错误策略。
- `src/metrics`、`src/sources`、`src/parsers`：数据采集与转换。
- `src/rendering`、`src/ui`：可复用卡片展示。
- `src/execution`：为用户操作和数据源提供有边界的子进程执行。
- `src/plugins`：可选外部集成。
- `docs/PET_CARD.md`：可选 Codex PetCard 的构建、hook 与资源配置。
- `docs/RUNTIME_POWER.md`：统一运行策略、省电行为和验证方法。
- `config`：可移植示例和卡片指南。
- `data`：桌面入口和应用图标。

## 安全与可移植性

命令使用明确的参数数组，并强制执行超时和输出限制。操作默认使用当前用户权限，
除非本地命令明确调用提权代理。仓库内默认配置不包含主机名、用户绝对路径、设备 ID、
凭据或特定机器优化。带身份验证的 HTTP 请求头应只写在被 Git 忽略的本地配置中，
不要提交到仓库。

## 许可证

MIT
