# PulseDeck

简体中文 | [English](README.md)

PulseDeck 是面向 Linux 手机、平板和桌面的轻量 GTK4/Libadwaita 配置化仪表盘。
页面、指标卡片、刷新计划、解析器和操作均可用 TOML 或 JSON 描述，因此大多数界面
调整无需重新编译应用。

## 功能

- 原生支持 CPU、内存、电池、功耗、网络、运行时间、文件系统、进程数、负载、
  交换空间、温度及网络吞吐指标。
- 支持内置指标、文件、命令、HTTP 和静态值数据源。
- 支持数值、进度、状态、文本、列表、组合和操作渲染器。
- 主数值统一使用直观格式：整百分比不显示无意义小数，单位自然排版，网络和功耗优先
  显示状态或功率。
- 支持固定间隔或 `daily@08:00,20:00` 等时间计划，并按时间槽缓存。
- 全局及单卡片响应式尺寸，适配移动端和桌面布局。
- 页面不可见时停止轮询。
- 统一管理前台正常、空闲省电、外接供电、后台及 Codex 事件唤醒状态，设置即时生效。
- 文件和网络状态事件驱动更新，临近刷新合并唤醒，共享系统快照并去重持久缓存写入。
- 限制子进程输出、HTTP 响应大小和执行时间。
- 可选、独立编译的 ScrcpyForge 设备控制页面。
- 可选、独立编译的 Codex PetCard，支持事件驱动动画、尺寸记忆和完成提示音。
- 页面工具栏可在普通网格与六列紧凑网格间切换，并记忆上次选择。

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

建议从 [config/config.example.toml](config/config.example.toml) 开始。仓库同时提供
内容一致的 [config/config.example.json](config/config.example.json)。当前 TOML schema
及实用卡片示例见 [config/CARD_GUIDE.md](config/CARD_GUIDE.md)。
PetCard 的构建、hook、动画、尺寸、功耗和提示音行为见
[docs/PET_CARD.md](docs/PET_CARD.md)。
统一运行模式、调度策略、插件适配和功耗验证方法见
[docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md)。

顶层配置包括：

- `[app]`：标题、日志、输出限制和配置重载。
- `[runtime]`：前台常亮、空闲显示与刷新、外接供电判断及 Codex 保护/通知。
- `[ui]`：默认页面、列数、卡片尺寸和紧凑布局。
- `[[pages]]`：按顺序排列的导航页面。
- `[[cards]]`：由可配置数据源提供内容的卡片。
- `[[actions]]`：用户主动触发的命令，可要求二次确认。

最小自定义卡片示例：

```toml
[[cards]]
id = "kernel"
title = "内核"
page = "monitor"
renderer = "text"
refresh_interval = 3600

[cards.source]
type = "command"
program = "uname"
args = ["-r"]
timeout_seconds = 5
```

当 `reload_on_change = true` 时，应用运行期间会重新读取数值类配置。新增或删除页面、
卡片后应重新打开应用，以重建完整页面结构。

## 数据源与渲染器

| 数据源 | 必填字段 | 用途 |
| --- | --- | --- |
| `builtin` | `metric` | 高效读取 Linux 原生系统指标。 |
| `file` | `path` | 读取文本、sysfs 或 procfs 文件。 |
| `command` | `program`，可选 `args` | 不经过 shell，运行有边界的子进程。 |
| `http` | `url`，可选方法/请求头/正文/解析器 | 获取本地或远程数据。 |
| `static_value` | `options.value` | 标签和固定信息卡片。 |

渲染器包括 `value`、`progress`、`status`、`text`、`list`、`composite` 和
`action`。应选择与数据源输出匹配的渲染器；内置指标已经返回相应结构化数值。

## 可选 ScrcpyForge 集成

默认构建不包含该集成。启用 `scrcpy-forge` feature，并将
`src/plugins/scrcpy_forge/config.example.toml` 中的通用配置追加到本地 TOML 文件。
它连接到单独安装的 ScrcpyForge 后端；PulseDeck 不持有 ADB 或 scrcpy 进程。服务
程序、URL 和脚本均可配置。预览与健康检查使用统一运行模式：页面隐藏或应用后台时
停止预览，空闲时只取设备元数据，未变化的画面通过 ETag 和内容哈希复用。

## 可选 Codex PetCard

`pet-card` feature 通过通用卡片插件接口接入，Codex 专属状态和定时器不会进入主线
核心。`integrations/pulsedeck-pet` 中可单独安装的 hook 只通过原子状态文件发布固定
生命周期状态，不读取提示词或工具内容。离线状态完全静止，卡片不可见时暂停动画，
应用后台时彻底停止帧定时器；完成提示音由全局运行设置控制。详见
[docs/PET_CARD.md](docs/PET_CARD.md)。

## 项目结构

- `src/core`：配置、运行/供电状态、调度、缓存和注册表。
- `src/metrics`、`src/sources`、`src/parsers`：数据采集与转换。
- `src/rendering`、`src/ui`：可复用卡片展示。
- `src/actions`：有执行边界的用户操作。
- `src/plugins`：可选外部集成。
- `docs/PET_CARD.md`：可选 Codex PetCard 的构建、hook 与资源配置。
- `docs/RUNTIME_POWER.md`：统一运行模式、省电策略和验证方法。
- `config`：可移植示例和卡片指南。
- `data`：桌面入口和应用图标。

## 安全与可移植性

命令使用明确的参数数组，并强制执行超时和输出限制。操作默认使用当前用户权限，
除非本地命令明确调用提权代理。仓库内默认配置不包含主机名、用户绝对路径、设备 ID、
凭据或特定机器优化。带身份验证的 HTTP 请求头应只写在被 Git 忽略的本地配置中，
不要提交到仓库。

## 许可证

MIT
