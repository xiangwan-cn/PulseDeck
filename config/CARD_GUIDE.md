# PulseDeck 卡片配置指南

编辑 `~/.config/pulsedeck/config.toml`，复制一个 `[[cards]]` 配置块并修改 `id`、
`title`、`order`、`renderer` 和数据源即可。内置渲染器包括 `value`、
`progress`、`status`、`text`、`list`、`composite`；数据源包括 `builtin`、
`file`、`command`、`http` 和 `static_value`。

例如，启用已内置但默认不显示的系统负载：

```toml
[[cards]]
id = "load"
title = "系统负载"
page = "monitor"
order = 100
renderer = "value"
refresh_interval = 30
enabled = true
icon = "utilities-system-monitor-symbolic"

[cards.source]
type = "builtin"
metric = "load_average"
```

可用的隐藏原生指标：

- `load_average`：1/5/15 分钟系统负载
- `swap`：交换空间占用
- `process_count`：进程数量
- `cpu_temperature`：CPU/SoC 温度
- `filesystem`：根文件系统占用
- `network_traffic`：上下行实时速率

任意 Shell、Python、Go 或独立 Rust 程序都可通过 `command` 数据源成为卡片，
因此自定义业务逻辑也不需要重新编译 PulseDeck。配置文件保存后会被重新读取；新增或
删除卡片后重新打开应用即可重建页面。

完整可复制配置见 `config/config.example.toml`；`config.example.json` 使用相同的
当前 schema。每个 `id` 应在自己的类型中保持唯一，卡片引用的 `page` 必须存在。

## 通用卡片字段

```toml
[[cards]]
id = "unique-id"              # 必填，稳定且唯一
title = "标题"                 # 必填
page = "monitor"              # 必填，对应 [[pages]].id
order = 10                    # 数值越小越靠前
renderer = "value"            # value/progress/status/text/list/composite/action
refresh_interval = 30         # 秒
enabled = true
icon = "computer-symbolic"    # 可选，Freedesktop 图标名
description = "说明"           # 可选
cache_ttl_seconds = 300        # 可选
# schedule = "daily@08:00,20:00"
```

`refresh_interval` 控制普通轮询间隔。设置 `schedule` 后，应用按每日固定时间生成独立
缓存周期。失败任务会进行有上限的退避，避免持续快速重试。

## 数据源示例

### 内置指标

内置指标不启动外部进程，适合系统监控：

```toml
[cards.source]
type = "builtin"
metric = "filesystem"
```

全部名称：`cpu`、`memory`、`uptime`、`battery_capacity`、
`battery_temperature`、`power`、`network`、`load_average`、`swap`、
`process_count`、`cpu_temperature`、`filesystem`、`network_traffic`。
没有电池或温度传感器的设备会显示不可用，不需要按机型修改配置。

### 文件

```toml
[cards.source]
type = "file"
path = "/proc/sys/kernel/hostname"

[cards.source.options]
first_line_only = true
```

文件源适合 procfs、sysfs 或普通文本。路径属于运行设备的本地配置；公开示例不要写入
个人主目录。

### 命令

```toml
[cards.source]
type = "command"
program = "uname"
args = ["-r"]
timeout_seconds = 5
max_output_bytes = 4096

[cards.source.options]
reverse_lines = false
max_subtitle_lines = 3
```

`program` 和 `args` 直接传给子进程，不经过 shell。需要管道、重定向或变量展开时，
应在自己的本地脚本中实现，并将脚本作为 `program`；不要把未经信任的内容拼入命令。
命令失败时，卡片只显示 stderr 最后一个非空行（例如 Python traceback 最后的
`RuntimeError` 消息），完整 stderr 保留在 tooltip 中，因此离线错误不会撑高卡片。

### HTTP

```toml
[cards.source]
type = "http"
url = "https://example.invalid/api/status"
method = "GET"
timeout_seconds = 10
max_output_bytes = 65536
headers = { Accept = "application/json" }

[cards.source.parser]
type = "json_path"
path = "data.status"
```

HTTP 方法支持 `GET`、`POST`、`PUT`、`DELETE` 和 `PATCH`。真实服务地址、认证请求头
和 token 只应写入被 Git 忽略的本地配置，不要提交到公开仓库。

### 静态值

```toml
[cards.source]
type = "static_value"

[cards.source.options]
value = "本地仪表盘"
```

静态值适合说明、分组提示或暂不需要轮询的数据。

## HTTP 解析器

- `json_path`：用点号访问对象或数组，如 `data.items.0.value`。
- `regex`：使用 `pattern` 和可选的 `capture` 提取文本。
- `number`：使用 `multiplier`、`divisor`、`decimal_places` 和 `suffix` 转为数值。
- `first_line`：只保留第一行，并可追加 `suffix`。

数值解析示例：

```toml
[cards.source.parser]
type = "number"
divisor = 1000
decimal_places = 1
suffix = "°C"
```

## 操作按钮

操作仅在用户点击时运行，不是周期卡片：

```toml
[[actions]]
id = "system-summary"
name = "查看系统信息"
description = "显示当前系统内核和架构"
icon = "utilities-system-monitor-symbolic"
page = "actions"
command = ["uname", "-a"]
timeout = 5
confirm = false
max_output_bytes = 4096
```

会修改系统状态的操作应设置 `confirm = true`。PulseDeck 不会自动提权；若本地操作
需要管理员权限，应由用户明确配置 `pkexec` 等授权工具。

## 卡片尺寸

默认使用三列、133 像素固定高度；横屏显示九张卡片时维持原来的铺满布局，
较长内容会自动切换为较小字号，不再把整行卡片撑高：

```toml
[ui]
card_columns = 3
card_height = 133
fixed_card_size = true
# card_width = 180 # 省略时自动等分可用宽度
```

单张卡片可以在 `[cards.display]` 中覆盖全局设置：

```toml
[cards.display]
card_width = 220
card_height = 160
fixed_size = false # false 表示内容较多时允许卡片继续增高
```

页面切换栏右上角的网格按钮可在默认列数和六列紧凑布局间切换。紧凑模式仍保留标题、
主要数值、顶部说明和底部状态文字，只隐藏占宽明显的图标与刷新按钮。选择通过
`${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/compact-grid` 原子保存，下次启动
及配置热重载后继续使用上次模式。

## 固定时间更新

命令、HTTP 和外部插件卡片都可以声明每日更新时间。应用会为每个时间点生成
独立缓存周期，同一周期最多执行一次；如果应用在时间点之后才启动，会补取当天
最近一期，而不会要求应用恰好在该分钟运行：

```toml
schedule = "daily@08:05,12:05,16:05,20:05"
cache_ttl_seconds = 14400
```

该机制不依赖卡片 ID、标题或脚本内容，任意定时联网或命令卡片都可直接复用。

## 可选 ScrcpyForge 页面

该页面不属于通用卡片系统，默认不会编译。使用
`cargo build --release --features scrcpy-forge` 启用，配置见
`src/plugins/scrcpy_forge/config.example.toml`。页面不可见时停止预览和健康请求；
重新进入后自动恢复。每台设备拥有对应的预览卡和脚本卡。

## 可选 PetCard

PetCard 默认不编译。使用 `cargo build --release --features pet-card` 启用；配置、
Codex hook、帧资源、四格/六格/全屏尺寸、离线回落和完成提示音见
[`docs/PET_CARD.md`](../docs/PET_CARD.md)。
